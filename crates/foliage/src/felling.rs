//! Chopping trees down.
//!
//! Damage (an axe, a saw, a bullet, an explosion) arrives as an `engine_world::DamageEvent`
//! aimed at a plant, one of its drawn parts or its trunk collider. Each hit adds chop damage;
//! when it passes the species' toughness (scaled by the tree's size) the tree is felled:
//!
//! 1. The plant becomes a stump (resprouting species such as oak, birch and hazel grow back
//!    from it).
//! 2. The trunk and crown above the cut hinge over away from the blow, accelerating like a
//!    real falling pole, and crash down: a `WorldImpulse` that shakes nearby foliage and kicks
//!    up dust, a `NoiseEvent` AI can hear, `GroundContact`s for audio and terrain, and
//!    `DamageEvent`s for whatever it lands on. Saplings and small plants under it are crushed.
//! 3. It settles as a [`FelledLog`] that sheds its leaves, greys and rots over years, feeding
//!    the soil where new trees sprout.
//!
//! Explosions (`WorldImpulse` with energy) knock trees over the same way, and standing dead or
//! burnt snags can be felled too.
//!
//! Every tree also gets a static trunk collider on a separate entity (marked
//! [`FoliageCollider`]) so players, vehicles and bullets hit trunks once a physics backend is
//! installed; logs get one when they settle.

use bevy_ecs::message::MessageCursor;
use engine_core::prelude::*;
use engine_render::GrassBender;
use engine_world::physics::{Collider, ColliderShape, PhysicsQueries, QueryFilter, RigidBody};
use engine_world::{
    BurnState, Combustible, ContactKind, DamageEvent, DamageKind, FocusPoints, Gravity, Ground,
    GroundContact, NoiseEvent, WorldImpulse,
};

use crate::meshgen::tree::cut_height;
use crate::plant::{EcologySettings, FelledLog, Genes, Plant, PlantStage};
use crate::species::{PlantLibrary, PlantSpecies, SpeciesId};
use crate::visuals::{PlantPart, plant_pose};

/// How a felled log lies and what it was: which way it fell (`yaw`, radians around +Y from
/// +X), the genes of the tree (its shape) and whether it was charred by fire.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LogPose {
    pub yaw: f32,
    pub genes: Genes,
    pub charred: bool,
}

/// A tree falling after its last chop: the part above the cut swings down around a hinge on
/// the stump. Becomes a [`FelledLog`] when it has landed and settled.
#[derive(Component, Debug, Clone)]
pub struct FallingTree {
    pub species: SpeciesId,
    pub size: f32,
    /// Where the stump stands (base of the tree).
    pub stump: DVec3,
    /// The cut, where the trunk pivots.
    pub hinge: DVec3,
    /// Angle from upright, radians.
    pub angle: f32,
    /// Angular speed, radians per second.
    pub speed: f32,
    /// Angle at which it hits the ground.
    pub land_angle: f32,
    /// Trunk length above the cut, meters.
    pub length: f32,
    pub mass_kg: f32,
    /// Years the wood had already been dead (a felled snag is grey and bare at once).
    pub years_dead: f32,
    /// After landing: blend from the impact pose to the resting pose (0..1).
    pub settle: Option<(f32, Transform)>,
}

/// On a child of a falling tree: presses the meadow grass under it ([`GrassBender`]) as the
/// trunk and crown come down, and lets go `linger_secs` after the log has settled.
#[derive(Component, Debug, Clone, Copy)]
pub struct GrassPress {
    pub linger_secs: f32,
}

/// A static collider owned by a plant or log (a separate entity, so the plant's non-uniform
/// scale never reaches the physics backend). Damage aimed at it chops its owner.
#[derive(Component, Debug, Clone, Copy)]
pub struct FoliageCollider {
    pub owner: Entity,
}

/// Link from a plant or log to its collider entity.
#[derive(Component, Debug, Clone, Copy)]
pub struct HasCollider(pub Entity);

/// A tree too far from every focus to need a trunk collider
/// ([`EcologySettings::collider_distance_m`]); it gets one again when a focus comes near.
#[derive(Component, Debug, Clone, Copy)]
pub struct ColliderStreamedOut;

/// Marks a log whose resting pose has been worked out.
#[derive(Component, Debug, Clone, Copy)]
pub struct LogPlaced;

/// What a chop did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChopResult {
    /// Not a plant, or nothing left to chop (a stump, a log).
    Missed,
    /// The blow didn't count (fire, cold) or wasn't enough yet: `progress` 0..1 toward felling.
    Damaged { progress: f32 },
    /// The tree fell; `log` is the falling trunk (a [`FallingTree`], later a [`FelledLog`]).
    Felled { log: Entity },
    /// A small plant or bush was cut down.
    Cut,
}

/// How much of a hit's damage counts as chopping.
pub fn chop_factor(kind: DamageKind) -> f32 {
    match kind {
        DamageKind::Slash => 1.0,
        DamageKind::Explosive => 1.5,
        DamageKind::Blunt => 0.35,
        DamageKind::Ballistic => 0.25,
        DamageKind::Pierce => 0.1,
        DamageKind::Magic => 0.5,
        DamageKind::Fire | DamageKind::Cold | DamageKind::Fall => 0.0,
    }
}

/// Chop damage needed to fell a tree of this size.
pub fn fell_threshold(sp: &PlantSpecies, size: f32) -> f32 {
    sp.toughness * size.max(0.05).powf(1.5)
}

/// The plant a damage target stands for: the plant itself, its trunk collider, or one of its
/// drawn parts.
pub fn plant_of(world: &World, target: Entity) -> Option<Entity> {
    let e = world.get_entity(target).ok()?;
    if e.contains::<Plant>() {
        return Some(target);
    }
    if let Some(c) = e.get::<FoliageCollider>() {
        return world.get::<Plant>(c.owner).map(|_| c.owner);
    }
    if e.contains::<PlantPart>() {
        let parent = e.get::<ChildOf>()?.parent();
        return world.get::<Plant>(parent).map(|_| parent);
    }
    None
}

/// Apply one blow to a plant (or its collider or part). `push` is the direction of the blow
/// (the tree falls that way); zero picks a direction from the tree's lean.
pub fn chop(
    world: &mut World,
    target: Entity,
    amount: f32,
    kind: DamageKind,
    push: Vec3,
) -> ChopResult {
    let Some(entity) = plant_of(world, target) else {
        return ChopResult::Missed;
    };
    let plant = world.get::<Plant>(entity).cloned().unwrap();
    let Some(library) = world.get_resource::<PlantLibrary>() else {
        return ChopResult::Missed;
    };
    let sp = library.get(plant.species).clone();
    let points = amount.max(0.0) * chop_factor(kind);
    if matches!(plant.stage, PlantStage::Stump { .. }) || points <= 0.0 {
        return ChopResult::Damaged { progress: 0.0 };
    }
    if !sp.is_woody() {
        // Grass, flowers, ferns and mushrooms: one decent blow cuts them.
        if points >= sp.toughness * plant.size.max(0.2) {
            world.entity_mut(entity).despawn();
            return ChopResult::Cut;
        }
        return ChopResult::Damaged {
            progress: points / (sp.toughness * plant.size.max(0.2)),
        };
    }
    let threshold = fell_threshold(&sp, plant.size);
    let damage = plant.chop_damage + points;
    if damage < threshold {
        if let Some(mut p) = world.get_mut::<Plant>(entity) {
            p.chop_damage = damage;
        }
        return ChopResult::Damaged {
            progress: damage / threshold,
        };
    }
    if !sp.is_tree() || plant.size < 0.2 {
        // Bushes and saplings are cut to the ground (bushes resprout).
        if let Some(mut p) = world.get_mut::<Plant>(entity) {
            p.stage = PlantStage::Stump { years: 0.0 };
            p.chop_damage = 0.0;
        }
        return ChopResult::Cut;
    }
    let mut dir = Vec3::new(push.x, 0.0, push.z);
    if dir.length_squared() < 1e-6 {
        let a = plant.genes.lean_dir;
        dir = Vec3::new(a.cos(), 0.0, a.sin());
    }
    match fell_tree(world, entity, dir.normalize()) {
        Some(log) => ChopResult::Felled { log },
        None => ChopResult::Missed,
    }
}

/// Ground height under `p` (flat ground at y = 0 when the world has none).
fn ground_y(ground: Option<&Ground>, p: DVec3) -> f64 {
    ground
        .and_then(|g| g.height(DVec3::new(p.x, p.y + 4.0, p.z)))
        .unwrap_or(0.0)
}

/// Height and cut height of a species' mesh at full size (model space, meters).
fn model_heights(sp: &PlantSpecies) -> (f32, f32) {
    (sp.max_height_m, cut_height(sp))
}

/// Fell a standing tree toward `dir` (horizontal, normalized). The plant becomes a stump and a
/// [`FallingTree`] is spawned. Returns the falling trunk.
pub fn fell_tree(world: &mut World, entity: Entity, dir: Vec3) -> Option<Entity> {
    let plant = world.get::<Plant>(entity)?.clone();
    let pos = world.get::<Transform>(entity)?.translation;
    let sp = world
        .get_resource::<PlantLibrary>()?
        .get(plant.species)
        .clone();
    if !sp.is_tree() || matches!(plant.stage, PlantStage::Stump { .. }) {
        return None;
    }
    let (height, cut) = model_heights(&sp);
    let (rotation, scale) = plant_pose(&plant);
    let hinge = pos + (rotation * (scale * Vec3::Y * cut)).as_dvec3();
    let length = ((height - cut) * scale.y).max(0.5);
    let yaw = dir.z.atan2(dir.x);
    let years_dead = match plant.stage {
        PlantStage::Dead { years } => years.max(1.5),
        PlantStage::Burnt { years } => years.max(1.5),
        _ => 0.0,
    };
    let charred = matches!(plant.stage, PlantStage::Burnt { .. });
    let mass_kg = sp.fuel_kg * plant.size * plant.size;

    // Where the crown hits the ground decides the landing angle.
    let ground = world.get_resource::<Ground>();
    let tip_ground = ground_y(ground, hinge + (dir * length).as_dvec3());
    let crown_support = sp.crown_radius_m * plant.size * plant.genes.crown * 0.12;
    let drop = (hinge.y - tip_ground - crown_support as f64) as f32;
    let land_angle = (drop / length).clamp(-0.6, 0.98).acos();

    if let Some(mut p) = world.get_mut::<Plant>(entity) {
        p.stage = PlantStage::Stump { years: 0.0 };
        p.chop_damage = 0.0;
    }
    if let Some(mut c) = world.get_mut::<Combustible>(entity) {
        c.fuel_kg *= 0.05;
    }
    let pose = LogPose {
        yaw,
        genes: plant.genes,
        charred,
    };
    let falling = FallingTree {
        species: plant.species,
        size: plant.size,
        stump: pos,
        hinge,
        angle: 0.02,
        speed: 0.08,
        land_angle,
        length,
        mass_kg,
        years_dead,
        settle: None,
    };
    let transform = falling_transform(&falling, &pose, &sp);
    let log = world
        .spawn((
            transform,
            TransformInterpolation::default(),
            falling,
            pose,
            Combustible {
                fuel_kg: mass_kg * 0.95,
                moisture: if years_dead > 0.0 { 0.15 } else { 0.45 },
                ignition_c: sp.ignition_c,
                temperature_c: 15.0,
                state: if charred {
                    BurnState::Burnt
                } else {
                    BurnState::Intact
                },
            },
        ))
        .id();
    // Grass parts under the trunk as it comes down and the crown presses it flat. Points on
    // the model's axis, so they follow the falling transform.
    let trunk_r = (sp.trunk_radius_m * plant.size * plant.genes.crown).max(0.15);
    let crown_r = (sp.crown_radius_m * plant.size * plant.genes.crown).max(0.8);
    let along = |s: f32| (cut + (height - cut) * s) as f64;
    let presses = [
        (along(0.12), trunk_r * 2.5),
        (along(0.32), trunk_r * 2.5),
        (along(0.52), trunk_r * 2.5),
        (along(0.74), crown_r),
    ];
    world.spawn_batch(presses.map(|(y, radius)| {
        (
            Transform::from_xyz(0.0, y, 0.0),
            GrassBender { radius },
            GrassPress { linger_secs: 4.0 },
            ChildOf(log),
        )
    }));
    world.write_message(NoiseEvent {
        position: hinge,
        radius: 30.0 + 40.0 * plant.size,
        source: Some(log),
    });
    tracing::debug!("foliage: felled {} ({:.0} m)", sp.name, length);
    Some(log)
}

/// The rotation that tips a tree from upright toward `yaw` by `angle`.
fn tip(yaw: f32, angle: f32) -> Quat {
    let dir = Vec3::new(yaw.cos(), 0.0, yaw.sin());
    Quat::from_axis_angle(Vec3::Y.cross(dir).normalize(), angle)
}

/// Spin of a plant's model around its own axis (the yaw part of [`plant_pose`]).
fn model_spin(genes: &Genes) -> Quat {
    let plant = Plant {
        genes: Genes {
            lean: 0.0,
            ..*genes
        },
        ..Default::default()
    };
    plant_pose(&plant).0
}

/// Years a log keeps its twigs; after that it is drawn bare (the plant's `log` mesh).
pub const LOG_BARE_YEARS: f32 = 1.2;

fn log_scale(genes: &Genes, size: f32) -> Vec3 {
    let s = size.max(0.02);
    Vec3::new(s * genes.crown, s * genes.height, s * genes.crown)
}

/// Transform of a falling tree at its current angle: the model point at the cut stays on the
/// hinge.
pub fn falling_transform(f: &FallingTree, pose: &LogPose, sp: &PlantSpecies) -> Transform {
    let (_, cut) = model_heights(sp);
    let scale = log_scale(&pose.genes, f.size);
    let rotation = tip(pose.yaw, f.angle) * model_spin(&pose.genes);
    let cut_point = rotation * (scale * Vec3::Y * cut);
    Transform {
        translation: f.hinge - cut_point.as_dvec3(),
        rotation,
        scale,
    }
}

/// Where a log comes to rest beside its stump: the butt slides off the stump onto the ground,
/// the crown props the top end up a little, and it lies along the slope. A log that has lain
/// `years` (with `rot` of its rotting done, 0..1) has lost its crown, so its top end lies on
/// the ground too, it has sunk into the litter, and its mossy side (the model's +X) is up.
pub fn log_rest_transform(
    ground: Option<&Ground>,
    stump: DVec3,
    pose: &LogPose,
    size: f32,
    sp: &PlantSpecies,
    years: f32,
    rot: f32,
) -> Transform {
    let (height, cut) = model_heights(sp);
    let scale = log_scale(&pose.genes, size);
    let dir = Vec3::new(pose.yaw.cos(), 0.0, pose.yaw.sin());
    let side = Vec3::Y.cross(dir).normalize();
    let r = (sp.trunk_radius_m * size * pose.genes.crown).max(0.04);
    let length = ((height - cut) * scale.y).max(0.5);
    let flip = if pose.genes.hue >= 0.0 { 1.0 } else { -1.0 };
    let old = years >= LOG_BARE_YEARS;
    let sink = 0.9 - 0.4 * rot.clamp(0.0, 1.0);
    let mut butt = stump + (side * r * 2.2 * flip + dir * r).as_dvec3();
    butt.y = ground_y(ground, butt) + (r * sink) as f64;
    let mut tip_p = butt + (dir * length).as_dvec3();
    let crown_support = if old {
        r * 0.35 * sink
    } else {
        (sp.crown_radius_m * size * pose.genes.crown * 0.12).max(r)
    };
    tip_p.y = ground_y(ground, tip_p) + crown_support as f64;
    let axis = (tip_p - butt).as_vec3().normalize_or(dir);
    let lying = Quat::from_rotation_arc(Vec3::Y, axis) * model_spin(&pose.genes);
    let roll = if old {
        // Turn the model's +X side (where the log mesh carries its moss) up, give or take.
        let x = lying * Vec3::X;
        let up = (Vec3::Y - axis * axis.y).normalize_or(Vec3::Y);
        axis.dot(x.cross(up)).atan2(x.dot(up)) + 0.4 * pose.genes.lean_dir.sin()
    } else {
        pose.genes.lean_dir * 3.1
    };
    let rotation = Quat::from_axis_angle(axis, roll) * lying;
    let cut_point = rotation * (scale * Vec3::Y * cut);
    Transform {
        translation: butt - cut_point.as_dvec3(),
        rotation,
        scale,
    }
}

/// Let the grass pressed by a falling tree spring back a few seconds after the log has settled
/// (by then the field no longer grows grass under the log).
pub fn release_grass_presses(
    mut commands: Commands,
    time: Res<Time>,
    mut presses: Query<(Entity, &ChildOf, &mut GrassPress)>,
    falling: Query<(), With<FallingTree>>,
) {
    let dt = time.delta_secs();
    for (e, parent, mut press) in &mut presses {
        if falling.contains(parent.parent()) {
            continue;
        }
        press.linger_secs -= dt;
        if press.linger_secs <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

/// Read blows and blasts and chop whatever they hit.
pub fn apply_damage(
    world: &mut World,
    mut cursors: Local<(MessageCursor<DamageEvent>, MessageCursor<WorldImpulse>)>,
) {
    let hits: Vec<DamageEvent> = match world.get_resource::<Messages<DamageEvent>>() {
        Some(m) => cursors.0.read(m).cloned().collect(),
        None => Vec::new(),
    };
    let blasts: Vec<WorldImpulse> = match world.get_resource::<Messages<WorldImpulse>>() {
        Some(m) => cursors
            .1
            .read(m)
            .copied()
            .filter(|b| b.energy_j > 0.0)
            .collect(),
        None => Vec::new(),
    };
    for hit in hits {
        let mut push = hit.impulse;
        if push.length_squared() < 1e-6
            && let Some(src) = hit.source.and_then(|s| world.get::<GlobalTransform>(s))
        {
            push = (hit.point - src.translation()).as_vec3();
        }
        if push.length_squared() < 1e-6
            && let Some(t) = world.get::<Transform>(hit.target)
        {
            push = (t.translation - hit.point).as_vec3();
        }
        chop(world, hit.target, hit.amount, hit.kind, push);
    }
    for blast in blasts {
        let r = blast.radius.max(0.1) as f64;
        let mut q = world.query_filtered::<(Entity, &Transform), With<Plant>>();
        let near: Vec<(Entity, DVec3)> = q
            .iter(world)
            .filter(|(_, t)| t.translation.distance(blast.center) < r)
            .map(|(e, t)| (e, t.translation))
            .collect();
        for (e, p) in near {
            let falloff = (1.0 - p.distance(blast.center) / r).powi(2) as f32;
            // Joules to chop points: a hand grenade (≈ 0.8 MJ) fells a mature oak within a
            // couple of meters and strips saplings further out.
            let points = blast.energy_j * falloff / 5000.0;
            if points > 0.5 {
                chop(
                    world,
                    e,
                    points,
                    DamageKind::Explosive,
                    (p - blast.center).as_vec3(),
                );
            }
        }
    }
}

/// Swing falling trees down, crash them and settle them into logs.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update_falling_trees(
    mut commands: Commands,
    time: Res<Time>,
    library: Res<PlantLibrary>,
    ground: Option<Res<Ground>>,
    gravity: Option<Res<Gravity>>,
    physics: Option<Res<PhysicsQueries>>,
    mut falling: Query<(Entity, &mut FallingTree, &LogPose, &mut Transform)>,
    mut plants: Query<(&mut Plant, &Transform), Without<FallingTree>>,
    own: Query<(), With<FoliageCollider>>,
    mut messages: (
        MessageWriter<WorldImpulse>,
        MessageWriter<NoiseEvent>,
        MessageWriter<GroundContact>,
        MessageWriter<DamageEvent>,
    ),
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let ground = ground.as_deref();
    for (entity, mut f, pose, mut transform) in &mut falling {
        let sp = library.get(f.species);
        if let Some((t, from)) = f.settle {
            let t = (t + dt / 0.45).min(1.0);
            let rest = log_rest_transform(ground, f.stump, pose, f.size, sp, f.years_dead, 0.0);
            let k = t * t * (3.0 - 2.0 * t);
            *transform = from.lerp(&rest, k);
            f.settle = Some((t, from));
            if t >= 1.0 {
                commands
                    .entity(entity)
                    .remove::<(FallingTree, TransformInterpolation)>()
                    .insert((
                        FelledLog {
                            species: f.species,
                            years: f.years_dead,
                            rot_years: 8.0 + 10.0 * f.size,
                            size: f.size,
                        },
                        LogPlaced,
                    ));
                spawn_log_collider(&mut commands, entity, &rest, sp, f.size, &pose.genes, 0.7);
            }
            continue;
        }
        let g = gravity
            .as_deref()
            .map(|g| g.at(f.hinge).length())
            .unwrap_or(9.81);
        // A uniform pole pivoting at one end: α = 3g / 2L · sin θ.
        let alpha = 1.5 * g / f.length * f.angle.sin();
        f.speed += alpha * dt;
        f.angle += f.speed * dt;
        if f.angle < f.land_angle {
            *transform = falling_transform(&f, pose, sp);
            continue;
        }
        // Impact.
        f.angle = f.land_angle;
        let impact = falling_transform(&f, pose, sp);
        *transform = impact;
        f.settle = Some((0.0, impact));
        let dir = Vec3::new(pose.yaw.cos(), 0.0, pose.yaw.sin());
        let speed = f.speed * f.length;
        let crown_r = (sp.crown_radius_m * f.size * pose.genes.crown).max(1.0);
        let along = |s: f32| {
            let p = f.hinge + (dir * f.length * s).as_dvec3();
            DVec3::new(p.x, ground_y(ground, p), p.z)
        };
        let crown = along(0.75);
        let (impulses, noises, contacts, damage) = &mut messages;
        impulses.write(WorldImpulse {
            center: crown,
            radius: crown_r * 1.6 + 2.0,
            strength: 3.0 + 5.0 * f.size,
            energy_j: 0.0,
            heat_c: 0.0,
            source: Some(entity),
        });
        noises.write(NoiseEvent {
            position: along(0.5),
            radius: 60.0 + 120.0 * f.size,
            source: Some(entity),
        });
        for s in [0.2, 0.5, 0.8] {
            contacts.write(GroundContact {
                position: along(s),
                normal: Vec3::Y,
                size: [crown_r.min(3.0), f.length / 3.0],
                yaw: GroundContact::yaw_of(dir),
                load_kg: f.mass_kg / 3.0,
                speed: speed * s,
                kind: ContactKind::Body,
                surface: None,
                entity: Some(entity),
            });
        }
        // Saplings and small plants under the trunk and crown are crushed.
        let a = along(0.05);
        let b = along(1.0);
        for (mut plant, t) in &mut plants {
            if !plant.stage.is_alive() {
                continue;
            }
            let other = library.get(plant.species);
            if other.is_tree() && plant.size > 0.3 {
                continue;
            }
            let p = t.translation;
            let ab = b - a;
            let s = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let d = DVec2::new(p.x - (a.x + ab.x * s), p.z - (a.z + ab.z * s)).length();
            let reach = if s > 0.55 { crown_r as f64 * 0.7 } else { 0.6 };
            if d < reach {
                plant.stage = PlantStage::Dead { years: 0.0 };
            }
        }
        // Whatever physical thing it lands on takes a blow.
        if let Some(physics) = physics.as_deref() {
            let mid = (a + b) * 0.5;
            let axis = (b - a).as_vec3();
            let shape = ColliderShape::Capsule {
                radius: (crown_r * 0.4).max(0.3),
                half_height: axis.length() * 0.5,
            };
            let rot = Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::X));
            let mut hit = Vec::new();
            physics.overlap(
                &shape,
                mid + DVec3::Y,
                rot,
                &QueryFilter::excluding(entity),
                &mut hit,
            );
            for target in hit {
                if own.contains(target) {
                    continue;
                }
                damage.write(DamageEvent {
                    target,
                    amount: f.mass_kg * speed * 0.004,
                    kind: DamageKind::Blunt,
                    point: mid,
                    impulse: -Vec3::Y * f.mass_kg * speed * 0.05,
                    source: Some(entity),
                });
            }
        }
    }
}

/// Lay logs that appeared without falling (a region's history, scenes, the API) on the ground.
pub fn place_logs(
    mut commands: Commands,
    library: Res<PlantLibrary>,
    ground: Option<Res<Ground>>,
    mut logs: Query<(Entity, &FelledLog, &LogPose, &mut Transform), Without<LogPlaced>>,
) {
    for (e, log, pose, mut t) in &mut logs {
        let sp = library.get(log.species);
        let rot = log.years / log.rot_years.max(0.1);
        let rest = log_rest_transform(
            ground.as_deref(),
            t.translation,
            pose,
            log.size,
            sp,
            log.years,
            rot,
        );
        *t = rest;
        commands.entity(e).insert(LogPlaced);
        let reach = if log.years >= LOG_BARE_YEARS {
            0.32
        } else {
            0.7
        };
        spawn_log_collider(&mut commands, e, &rest, sp, log.size, &pose.genes, reach);
    }
}

/// A capsule along the log from its butt, `reach` of the trunk above the cut long.
fn spawn_log_collider(
    commands: &mut Commands,
    log: Entity,
    rest: &Transform,
    sp: &PlantSpecies,
    size: f32,
    genes: &Genes,
    reach: f32,
) {
    let (height, cut) = model_heights(sp);
    let r = (sp.trunk_radius_m * size * genes.crown).max(0.04);
    let axis = rest.rotation * Vec3::Y;
    let length = (height - cut) * rest.scale.y * reach;
    let butt = rest.translation + (rest.rotation * (rest.scale * Vec3::Y * cut)).as_dvec3();
    let collider = commands
        .spawn((
            Transform::from_translation(butt + (axis * length * 0.5).as_dvec3())
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, axis)),
            RigidBody::fixed(),
            Collider::capsule(r * 0.8, (length * 0.5 - r).max(0.05)).with_density(600.0),
            FoliageCollider { owner: log },
        ))
        .id();
    commands.entity(log).insert(HasCollider(collider));
}

/// Give trees a trunk collider and keep it matched to their size and stage.
#[allow(clippy::type_complexity)]
pub fn sync_tree_colliders(
    mut commands: Commands,
    library: Res<PlantLibrary>,
    plants: Query<
        (Entity, &Plant, &Transform, Option<&HasCollider>),
        (
            Or<(Changed<Plant>, Without<HasCollider>)>,
            Without<ColliderStreamedOut>,
        ),
    >,
    mut colliders: Query<(&mut Collider, &mut Transform), (With<FoliageCollider>, Without<Plant>)>,
) {
    for (e, plant, t, has) in &plants {
        let sp = library.get(plant.species);
        if !sp.is_tree() || plant.size < 0.15 {
            continue;
        }
        let r = (sp.trunk_radius_m * plant.size * plant.genes.crown).max(0.03);
        let (height, cut) = model_heights(sp);
        let scale_y = plant.size * plant.genes.height;
        let h = match plant.stage {
            PlantStage::Stump { .. } => cut * scale_y,
            _ => height * scale_y * 0.6,
        }
        .max(2.0 * r + 0.05);
        let shape = Collider::capsule(r, (h * 0.5 - r).max(0.02)).with_density(700.0);
        let pose = Transform::from_translation(t.translation + DVec3::Y * (h as f64 * 0.5));
        match has {
            Some(h) => {
                if let Ok((mut c, mut ct)) = colliders.get_mut(h.0)
                    && (c.shape != shape.shape || ct.translation != pose.translation)
                {
                    c.shape = shape.shape;
                    *ct = pose;
                }
            }
            None => {
                let c = commands
                    .spawn((
                        pose,
                        RigidBody::fixed(),
                        shape,
                        FoliageCollider { owner: e },
                    ))
                    .id();
                commands.entity(e).insert(HasCollider(c));
            }
        }
    }
}

/// Stream trunk colliders: trees far from every focus drop theirs, and get one back when a
/// focus comes within [`EcologySettings::collider_distance_m`] (a valley of thousands of trees
/// keeps only the nearby few hundred in the physics world).
#[allow(clippy::type_complexity)]
pub fn stream_tree_colliders(
    mut commands: Commands,
    mut timer: Local<f32>,
    time: Res<Time>,
    library: Res<PlantLibrary>,
    settings: Res<EcologySettings>,
    focus: Option<Res<FocusPoints>>,
    plants: Query<(
        Entity,
        &Plant,
        &Transform,
        Option<&HasCollider>,
        Has<ColliderStreamedOut>,
    )>,
) {
    *timer -= time.real_delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.5;
    let focus = focus.filter(|f| !f.points.is_empty());
    let r = settings.collider_distance_m as f64;
    for (e, plant, t, has, out) in &plants {
        let d = focus.as_ref().map_or(0.0, |f| f.distance(t.translation));
        if !out && d > r + 20.0 && library.get(plant.species).is_tree() {
            if let Some(h) = has {
                commands.entity(h.0).despawn();
                commands.entity(e).remove::<HasCollider>();
            }
            commands.entity(e).insert(ColliderStreamedOut);
        } else if out && d < r {
            commands.entity(e).remove::<ColliderStreamedOut>();
        }
    }
}

/// Remove colliders whose plant or log is gone.
#[allow(clippy::type_complexity)]
pub fn remove_orphan_colliders(
    mut commands: Commands,
    mut frame: Local<u32>,
    colliders: Query<(Entity, &FoliageCollider)>,
    owners: Query<(), Or<(With<Plant>, With<FelledLog>)>>,
) {
    *frame = frame.wrapping_add(1);
    if !frame.is_multiple_of(30) {
        return;
    }
    for (e, c) in &colliders {
        if !owners.contains(c.owner) {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.add_plugins(crate::FoliagePlugin);
        app
    }

    fn app_with_oak(size: f32) -> (App, Entity) {
        let mut app = test_app();
        let lib = PlantLibrary::default();
        let oak = lib.id("oak").unwrap();
        let tree = app
            .world
            .spawn((
                Transform::from_xyz(3.0, 0.0, 4.0),
                Plant::new(oak, size, Genes::default()),
            ))
            .id();
        (app, tree)
    }

    #[test]
    fn chopping_fells_a_tree_into_a_stump_and_a_log() {
        let (mut app, tree) = app_with_oak(0.8);
        app.update();
        let mut hits = 0;
        let log = loop {
            hits += 1;
            assert!(hits < 40, "the oak never fell");
            match chop(&mut app.world, tree, 25.0, DamageKind::Slash, Vec3::X) {
                ChopResult::Felled { log } => break log,
                ChopResult::Damaged { progress } => assert!(progress > 0.0 && progress < 1.0),
                other => panic!("unexpected {other:?}"),
            }
        };
        assert!(
            hits > 2,
            "a mature oak should take several blows, took {hits}"
        );
        assert!(matches!(
            app.world.get::<Plant>(tree).unwrap().stage,
            PlantStage::Stump { .. }
        ));
        // It falls toward +X (the push), lands and becomes a log within a few seconds.
        for _ in 0..600 {
            app.update();
            if app.world.get::<FelledLog>(log).is_some() {
                break;
            }
        }
        assert!(
            app.world.get::<FelledLog>(log).is_some(),
            "the tree never landed"
        );
        let t = app.world.get::<Transform>(log).unwrap();
        let axis = t.rotation * Vec3::Y;
        assert!(axis.x > 0.9, "log axis {axis:?} should point along +X");
        assert!(t.translation.y < 1.0);
        assert!(app.world.get::<HasCollider>(log).is_some());
    }

    #[test]
    fn grass_is_cut_by_one_blow_and_fire_does_not_chop() {
        let mut app = test_app();
        let lib = PlantLibrary::default();
        let grass = app
            .world
            .spawn((
                Transform::default(),
                Plant::new(lib.id("grass").unwrap(), 0.8, Genes::default()),
            ))
            .id();
        let oak = app
            .world
            .spawn((
                Transform::from_xyz(5.0, 0.0, 0.0),
                Plant::new(lib.id("oak").unwrap(), 0.9, Genes::default()),
            ))
            .id();
        assert_eq!(
            chop(&mut app.world, oak, 1000.0, DamageKind::Fire, Vec3::X),
            ChopResult::Damaged { progress: 0.0 }
        );
        assert_eq!(
            chop(&mut app.world, grass, 10.0, DamageKind::Slash, Vec3::X),
            ChopResult::Cut
        );
        assert!(app.world.get_entity(grass).is_err());
    }

    #[test]
    fn damage_events_on_the_trunk_collider_chop_the_tree() {
        let (mut app, tree) = app_with_oak(0.5);
        app.update();
        let collider = app
            .world
            .get::<HasCollider>(tree)
            .expect("trees get a trunk collider")
            .0;
        for _ in 0..30 {
            app.world.write_message(DamageEvent {
                target: collider,
                amount: 30.0,
                kind: DamageKind::Slash,
                point: DVec3::new(3.0, 1.0, 4.0),
                impulse: Vec3::Z,
                source: None,
            });
            app.update();
            if matches!(
                app.world.get::<Plant>(tree).unwrap().stage,
                PlantStage::Stump { .. }
            ) {
                break;
            }
        }
        assert!(matches!(
            app.world.get::<Plant>(tree).unwrap().stage,
            PlantStage::Stump { .. }
        ));
    }

    #[test]
    fn a_blast_knocks_nearby_trees_over() {
        let (mut app, tree) = app_with_oak(0.7);
        app.update();
        app.world.write_message(WorldImpulse {
            center: DVec3::new(2.0, 0.5, 4.0),
            radius: 12.0,
            strength: 40.0,
            energy_j: 2.0e6,
            heat_c: 0.0,
            source: None,
        });
        app.update();
        assert!(matches!(
            app.world.get::<Plant>(tree).unwrap().stage,
            PlantStage::Stump { .. }
        ));
        let mut q = app.world.query::<&FallingTree>();
        assert_eq!(q.iter(&app.world).count(), 1);
    }

    #[derive(Resource, Default)]
    struct Contacts(Vec<GroundContact>);

    fn collect_contacts(mut seen: ResMut<Contacts>, mut contacts: MessageReader<GroundContact>) {
        seen.0.extend(contacts.read().copied());
    }

    #[test]
    fn a_fallen_trunk_presses_the_ground_along_its_length() {
        let (mut app, tree) = app_with_oak(0.8);
        app.init_resource::<Contacts>()
            .add_systems(Update, collect_contacts);
        app.update();
        let dir = Vec3::new(0.6, 0.0, 0.8);
        let log = fell_tree(&mut app.world, tree, dir).expect("the oak falls");
        app.update_n(400);
        let pose = *app.world.get::<LogPose>(log).unwrap();
        let fall = Vec3::new(pose.yaw.cos(), 0.0, pose.yaw.sin());
        let seen = &app.world.resource::<Contacts>().0;
        let body: Vec<_> = seen
            .iter()
            .filter(|c| c.entity == Some(log) && c.kind == ContactKind::Body)
            .collect();
        assert_eq!(body.len(), 3);
        for c in body {
            assert!(c.forward().dot(fall) > 0.99, "{:?} vs {fall}", c.forward());
        }
    }

    #[test]
    fn a_falling_tree_presses_the_grass_until_it_has_settled() {
        let (mut app, tree) = app_with_oak(0.8);
        app.update();
        let log = fell_tree(&mut app.world, tree, Vec3::X).expect("the oak falls");
        let presses = |app: &mut App| -> Vec<(DVec3, f32)> {
            app.world
                .query::<(&GrassBender, &GlobalTransform, &ChildOf)>()
                .iter(&app.world)
                .filter(|(_, _, c)| c.parent() == log)
                .map(|(b, g, _)| (g.translation(), b.radius))
                .collect()
        };
        app.update_n(2);
        let standing = presses(&mut app);
        assert_eq!(standing.len(), 4);
        // Upright: stacked up the trunk, the widest one in the crown.
        assert!(standing.iter().all(|(p, _)| (p.x - 3.0).abs() < 0.5));
        assert!(standing[3].1 > standing[0].1 * 2.0);
        let mut frames = 0;
        while app.world.get::<FelledLog>(log).is_none() {
            frames += 1;
            assert!(frames < 600, "the tree never landed");
            app.update();
        }
        // Down: lying along +X near the ground, still pressing.
        let down = presses(&mut app);
        assert_eq!(down.len(), 4);
        assert!(down.iter().all(|(p, _)| p.y < 3.0), "{down:?}");
        assert!(down[3].0.x > down[0].0.x + 3.0, "{down:?}");
        app.update_n(5 * 60);
        assert!(presses(&mut app).is_empty());
    }

    #[test]
    fn old_logs_lie_low_with_their_mossy_side_up() {
        let lib = PlantLibrary::default();
        let sp = lib.get(lib.id("oak").unwrap());
        for k in 0..12 {
            let pose = LogPose {
                yaw: k as f32 * 0.7,
                genes: Genes {
                    lean_dir: k as f32 * 1.3,
                    hue: if k % 2 == 0 { 0.4 } else { -0.4 },
                    ..Default::default()
                },
                charred: false,
            };
            let fresh = log_rest_transform(None, DVec3::ZERO, &pose, 0.8, sp, 0.0, 0.0);
            let old = log_rest_transform(None, DVec3::ZERO, &pose, 0.8, sp, 6.0, 0.4);
            let rotten = log_rest_transform(None, DVec3::ZERO, &pose, 0.8, sp, 12.0, 0.9);
            // Within half a radian of straight up.
            assert!((old.rotation * Vec3::X).y > 0.4f32.cos() * 0.98, "{k}");
            let tip = |t: &Transform| t.translation + (t.rotation * Vec3::Y * 10.0).as_dvec3();
            assert!(
                tip(&old).y < tip(&fresh).y,
                "the crown no longer props it up"
            );
            assert!(
                rotten.translation.y < old.translation.y,
                "it sinks as it rots"
            );
        }
    }

    #[test]
    fn trunk_colliders_stream_with_the_focus() {
        let (mut app, near) = app_with_oak(0.7);
        let lib = PlantLibrary::default();
        let far = app
            .world
            .spawn((
                Transform::from_xyz(400.0, 0.0, 0.0),
                Plant::new(lib.id("oak").unwrap(), 0.7, Genes::default()),
            ))
            .id();
        app.world
            .resource_mut::<EcologySettings>()
            .collider_distance_m = 100.0;
        let focus = app
            .world
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                engine_world::SimulationFocus::default(),
            ))
            .id();
        app.update_n(40);
        assert!(app.world.get::<HasCollider>(near).is_some());
        assert!(app.world.get::<HasCollider>(far).is_none());
        // The focus walks over to the far tree: the colliders swap.
        *app.world.get_mut::<Transform>(focus).unwrap() = Transform::from_xyz(390.0, 0.0, 0.0);
        app.update_n(40);
        assert!(app.world.get::<HasCollider>(near).is_none());
        assert!(app.world.get::<HasCollider>(far).is_some());
        let mut q = app.world.query::<&FoliageCollider>();
        assert_eq!(q.iter(&app.world).count(), 1);
    }
}
