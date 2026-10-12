#![doc = include_str!("../README.md")]

pub mod animator;
pub mod attach;
pub mod build;
pub mod clip;
pub mod demo;
pub mod events;
pub mod graph;
pub mod ik;
pub mod import;
pub mod muscle;
pub mod procedural;
pub mod skeleton;
pub mod skin;
pub mod xform;

pub use animator::{AnimationLibrary, Animator, AnimatorSettings, PoseEdit};
pub use attach::BoneAttachment;
pub use clip::AnimationClip;
pub use graph::{AnimGraph, NodeDef};
pub use ik::{FootIk, IkShape, IkSurface, LegIk, LookAt, Reach, ReachIk};
pub use muscle::{MorphWeights, Muscle, MuscleDriver, MuscleRig};
pub use procedural::gait::{GaitLeg, GaitStage, LeggedGait};
pub use procedural::spring::{SpringChainDef, SpringChains};
pub use procedural::vehicle::{VehicleRig, WheelRig};
pub use skeleton::Skeleton;
pub use skin::feature::SkinningFeature;
pub use skin::{SkinMaterial, SkinPose, SkinnedMesh, SkinnedMeshRenderer};

use engine_assets::AssetAppExt;
use engine_core::prelude::*;
use engine_core::registry::resolve_entity;
use engine_render::RenderAppExt;
use engine_world::animation::{AnimationEvent, AnimationParams, AnimationTrigger};
use engine_world::{GroundContact, NoiseEvent};
use serde_json::{Value, json};

/// Order of the animation systems.
///
/// In `Update` (after gameplay logic has moved things): [`AnimSet::Setup`] builds animators
/// and queues triggers, [`AnimSet::Graph`] advances clips and graphs (and applies root
/// motion). In `PostUpdate`, once world transforms are final: [`AnimSet::Procedural`] (gaits,
/// vehicle suspension, spring chains), [`AnimSet::Ik`] (feet, look-at, reach),
/// [`AnimSet::Muscles`], then [`AnimSet::Output`] publishes `SkeletonPose`, skinning
/// matrices, bone attachments and events.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimSet {
    Setup,
    Graph,
    Procedural,
    Ik,
    Muscles,
    Output,
}

/// Skeletal and procedural animation: clips, blend graphs and state machines, IK, legged
/// gaits, vehicle rigs, spring chains, muscles, GPU skinning with cloth tension.
pub struct AnimationPlugin;

impl Plugin for AnimationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimationLibrary>()
            .init_resource::<ik::IkSurfaces>()
            .init_asset::<SkinnedMesh>()
            // Written and read here; registering them again is a no-op with WorldPlugin.
            .add_message::<AnimationTrigger>()
            .add_message::<AnimationEvent>()
            .add_message::<GroundContact>()
            .add_message::<NoiseEvent>()
            .add_render_feature(SkinningFeature::default());
        register_types(app);
        register_api(app);
        demo::register(app);

        app.configure_sets(
            Update,
            (AnimSet::Setup, AnimSet::Graph)
                .chain()
                .after(UpdateSet::Logic)
                .before(UpdateSet::Camera),
        )
        .configure_sets(
            PostUpdate,
            (
                AnimSet::Procedural,
                AnimSet::Ik,
                AnimSet::Muscles,
                AnimSet::Output,
            )
                .chain()
                .in_set(UpdateSet::AfterPropagate),
        )
        .add_systems(
            Update,
            (
                (animator::build_animators, animator::queue_triggers)
                    .chain()
                    .in_set(AnimSet::Setup),
                animator::evaluate_animators.in_set(AnimSet::Graph),
            ),
        )
        .add_systems(
            PostUpdate,
            (
                (
                    ik::gather_surfaces,
                    (
                        procedural::gait::gait_system,
                        procedural::vehicle::vehicle_rig_system,
                    ),
                    procedural::spring::spring_chain_system,
                )
                    .chain()
                    .in_set(AnimSet::Procedural),
                (ik::foot_ik_system, ik::look_at_system, ik::reach_ik_system)
                    .chain()
                    .in_set(AnimSet::Ik),
                muscle::muscle_system.in_set(AnimSet::Muscles),
                (
                    animator::publish_pose,
                    (
                        skin::update_skin_poses,
                        attach::attachment_system,
                        events::emit_events,
                    ),
                )
                    .chain()
                    .in_set(AnimSet::Output),
            ),
        );
    }
}

fn register_types(app: &mut App) {
    app.register_component::<AnimatorSettings>(
        "Animated character: skeleton and graph names from the AnimationLibrary, playback \
         speed, pause, root motion, LOD distance (m).",
    )
    .register_component::<FootIk>(
        "Plants feet on uneven ground: pelvis drop, two-bone legs, foot tilt (meters, degrees).",
    )
    .register_component::<LookAt>(
        "Turns spine, neck and head toward a world point (max angle in degrees, smoothing s).",
    )
    .register_component::<ReachIk>(
        "Two-bone IK chains that reach hands to world points or to points on other bones \
         (two-handed grips); a clip curve can fade each chain in and out.",
    )
    .register_component::<IkSurface>(
        "Box, ramp or stairs (half extents, m) that feet and gaits step on without physics.",
    )
    .register_component::<SpringChains>(
        "Tails, manes, ponytails, antennae: bone chains that lag, swing with gravity and wind.",
    )
    .register_component::<LeggedGait>(
        "Procedural legs for any number of legs: gait stages by speed (m/s), step height (m), \
         body follow and tilt, sways.",
    )
    .register_component::<VehicleRig>(
        "Vehicle body and wheel animation: suspension travel (m), frequency (Hz), damping, \
         roll and pitch per g (deg), steering (deg), wheel spin.",
    )
    .register_component::<MuscleRig>(
        "Muscles that flex from joint angles and effort, relax slowly and jiggle when slack \
         (strength, jiggle, boost 0..1).",
    )
    .register_component::<SkinnedMeshRenderer>(
        "Draws a skinned mesh deformed by the Animator, with cloth wrinkles, bunching, veins \
         and sheen (detail distance, m).",
    )
    .register_component::<BoneAttachment>(
        "Follows a bone of another entity (props, weapons): bone name, offset (m), rotation.",
    );
}

fn entity_arg(world: &mut World, args: &Value) -> Result<Entity, String> {
    let selector = args
        .get("entity")
        .ok_or("pass the character as \"entity\" (an id or a Name)")?;
    resolve_entity(world, selector)
}

fn register_api(app: &mut App) {
    app.register_api(
        "anim.list",
        "Every animated entity: skeleton, graph, active states and parameters.",
        "{}",
        |world, _| {
            let mut out = Vec::new();
            let mut q = world.query::<(Entity, &Animator, Option<&AnimationParams>, Option<&Name>)>();
            for (e, a, params, name) in q.iter(world) {
                out.push(json!({
                    "id": e.to_bits(),
                    "name": name.map(|n| n.as_str().to_string()),
                    "skeleton": a.skeleton_name(),
                    "graph": a.graph_name(),
                    "states": a.graph().map(|g| g.active_states()).unwrap_or_default(),
                    "params": params.map(|p| json!(p.values)).unwrap_or(Value::Null),
                }));
            }
            Ok(json!({ "characters": out }))
        },
    )
    .register_api(
        "anim.set_param",
        "Set an animation parameter (speed, turn, blocking, effort…) on a character.",
        r#"{"entity": "Athlete", "name": "speed", "value": 1.4}"#,
        |world, args| {
            let e = entity_arg(world, &args)?;
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("pass the parameter as \"name\"")?;
            let value = args
                .get("value")
                .and_then(Value::as_f64)
                .ok_or("pass a number as \"value\"")? as f32;
            let mut ent = world.get_entity_mut(e).map_err(|_| "no such entity".to_string())?;
            match ent.get_mut::<AnimationParams>() {
                Some(mut p) => p.set(name, value),
                None => {
                    ent.insert(AnimationParams::default());
                    if let Some(mut p) = ent.get_mut::<AnimationParams>() {
                        p.set(name, value);
                    }
                }
            }
            Ok(json!({ "ok": true }))
        },
    )
    .register_api(
        "anim.trigger",
        "Fire a graph trigger (heavy_swing, block, reach, flex…) on a character.",
        r#"{"entity": "Athlete", "name": "heavy_swing"}"#,
        |world, args| {
            let e = entity_arg(world, &args)?;
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("pass the trigger as \"name\"")?;
            let mut a = world
                .get_mut::<Animator>(e)
                .ok_or("that entity has no Animator")?;
            a.trigger(name);
            Ok(json!({ "ok": true }))
        },
    )
    .register_api(
        "anim.state",
        "A character's graph state, curves, events this frame and muscle activations.",
        r#"{"entity": "Athlete"}"#,
        |world, args| {
            let e = entity_arg(world, &args)?;
            let a = world.get::<Animator>(e).ok_or("that entity has no Animator")?;
            let mut out = json!({
                "skeleton": a.skeleton_name(),
                "graph": a.graph().map(|g| g.describe()).unwrap_or(Value::Null),
                "curves": a.curves.iter().map(|(n, v)| json!({"name": n, "value": v})).collect::<Vec<_>>(),
                "events": a.events.iter().map(|ev| json!({"name": ev.name, "bone": ev.bone})).collect::<Vec<_>>(),
            });
            if let Some(m) = world.get::<MuscleRig>(e) {
                out["muscles"] = json!(
                    m.activations()
                        .into_iter()
                        .map(|(n, v)| json!({"name": n, "activation": v}))
                        .collect::<Vec<_>>()
                );
            }
            if let Some(g) = world.get::<LeggedGait>(e) {
                out["gait"] = json!({"stage": g.stage_name(), "speed": g.speed(), "phase": g.phase()});
            }
            Ok(out)
        },
    )
    .register_api(
        "anim.clips",
        "Clips, skeletons and graphs in the animation library.",
        "{}",
        |world, _| {
            let lib = world.resource::<AnimationLibrary>();
            let mut clips: Vec<Value> = lib
                .clips
                .values()
                .map(|c| {
                    json!({
                        "name": c.name,
                        "duration": c.duration,
                        "looping": c.looping,
                        "events": c.events.iter().map(|e| json!({"time": e.time, "name": e.name})).collect::<Vec<_>>(),
                        "curves": c.curves.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            clips.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            let mut skeletons: Vec<&String> = lib.skeletons.keys().collect();
            skeletons.sort();
            let mut graphs: Vec<&String> = lib.graphs.keys().collect();
            graphs.sort();
            Ok(json!({ "clips": clips, "skeletons": skeletons, "graphs": graphs }))
        },
    )
    .register_api(
        "anim.add_graph",
        "Add or replace an animation graph (JSON, see the graph module docs). Characters \
         using it pick it up when their AnimatorSettings change.",
        r#"{"name": "my_graph", "root": {"type": "clip", "clip": "walk"}}"#,
        |world, args| {
            let graph: AnimGraph = serde_json::from_value(args)
                .map_err(|e| format!("not a valid animation graph: {e}"))?;
            let name = graph.name.clone();
            world.resource_mut::<AnimationLibrary>().add_graph(graph);
            Ok(json!({ "ok": true, "name": name }))
        },
    )
    .register_api(
        "anim.set_graph",
        "Switch a character to another graph from the library (validated first).",
        r#"{"entity": "Athlete", "graph": "humanoid"}"#,
        |world, args| {
            let e = entity_arg(world, &args)?;
            let graph = args
                .get("graph")
                .and_then(Value::as_str)
                .ok_or("pass the graph name as \"graph\"")?
                .to_string();
            let skeleton = world
                .get::<Animator>(e)
                .map(|a| a.skeleton_name().to_string())
                .ok_or("that entity has no Animator")?;
            let animator = world
                .resource::<AnimationLibrary>()
                .instantiate(&skeleton, &graph)?;
            let mut ent = world.entity_mut(e);
            ent.insert(animator);
            if let Some(mut s) = ent.get_mut::<AnimatorSettings>() {
                s.graph = graph;
            }
            Ok(json!({ "ok": true }))
        },
    );
}
