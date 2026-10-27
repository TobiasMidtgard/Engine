//! Rigged models: a placed glTF (or converted FBX) with a skin plays as an animated character
//! instead of a still node tree, the way Unity puts an Animator on an imported rig and Unreal
//! makes a Skeletal Mesh. The file goes through the anim crate's importer (skeleton, skinned
//! mesh, materials, every clip), each clip becomes a one-clip graph, and the placed root gets a
//! `SkinnedMeshRenderer` and an `AnimatorSettings` whose graph names the clip. Scenes save the
//! root with its `Model` and `AnimatorSettings`; the mesh is put back from the file on load.
//!
//! Animation-only files (a clip library exported against the same skeleton, as Mixamo and
//! Rocketbox ship them) dropped on a rig in the scene view add their clips to it and play,
//! the way dragging a clip onto a character in Unity's Scene view gives it an Animator. Bones
//! are matched by name.
//!
//! In edit mode the world holds still, but a rig previews its clip (Unreal's Persona and
//! Unity's Inspector preview play clips without entering Play), unless that's switched off in
//! Details.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use engine::assets::AssetServer;
use engine::prelude::*;
use engine::render::{Image, StandardMaterial};
use engine_anim::import::{ImportOptions, import_clips, import_gltf};
use engine_anim::skin::{SkinnedMesh, SkinnedMeshRenderer};
use engine_anim::{
    AnimGraph, AnimationClip, AnimationLibrary, Animator, AnimatorSettings, NodeDef,
};

use crate::editor::{self, Editor, Mode};
use crate::models::Model;
use crate::scene_ops::InScene;
use crate::undo::Command;

/// A model file loaded as a rig, shared by every placed copy.
pub struct Rig {
    /// Skeleton name in the [`AnimationLibrary`].
    pub skeleton: String,
    pub mesh: Handle<SkinnedMesh>,
    pub surfaces: Vec<StandardMaterial>,
    /// (clip name as the file has it, graph name in the library), in file order.
    pub clips: Vec<(String, String)>,
    /// Bind pose bounds in the root's space (min, max), scale applied.
    pub bounds: (Vec3, Vec3),
    pub bones: usize,
    /// File units → meters factor of the import (clip files are read with the same).
    pub unit_scale: f32,
}

/// Files loaded as rigs (or found to have no skin), by asset key and import scale.
#[derive(Resource, Default)]
pub struct Rigs {
    loaded: HashMap<String, Option<Arc<Rig>>>,
    /// Clips of a clip file read onto a rig, by "skeleton|file".
    clip_files: HashMap<String, Vec<(String, String)>>,
    /// Whether a file holds animations and nothing to draw.
    clip_only: HashMap<String, bool>,
    /// Travel speed (m/s) a clip's in-place copy lost, by graph name.
    travel: HashMap<String, f32>,
}

impl Rigs {
    /// Forget a file so the next use reads it again (reimport).
    pub fn forget(&mut self, key: &str) {
        let file = engine::assets::normalize_path(key);
        self.loaded
            .retain(|k, _| k.rsplit_once('@').map_or(k.as_str(), |(f, _)| f) != file);
        self.clip_files.retain(|k, _| {
            !k.starts_with(&format!("{file}@")) && !k.ends_with(&format!("|{file}"))
        });
        self.clip_only.remove(&file);
    }
}

/// How a placed rig plays its clips: clips from other files (asset keys: animation files
/// dropped on it), and whether walks play in place.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelClips {
    pub files: Vec<String>,
    /// Play every clip with its root's travel taken out (Unity's "Bake Into Pose").
    pub in_place: bool,
}

/// This placed model's root shows its file as a rig (`animated`) or as a still node tree. Not
/// saved: scenes are matched up again when they load.
#[derive(Component, Debug, Clone)]
pub struct Rigged {
    pub animated: bool,
    /// Play the clip while editing.
    pub preview: bool,
}

/// The rig for `key` at `scale`: `Ok(None)` when the file has no skin.
pub fn rig(world: &mut World, key: &str, scale: f64) -> Result<Option<Arc<Rig>>, String> {
    let file = engine::assets::normalize_path(key);
    let id = format!("{file}@{scale}");
    if let Some(found) = world.get_resource::<Rigs>().and_then(|r| r.loaded.get(&id)) {
        return Ok(found.clone());
    }
    let root = world
        .get_resource::<AssetServer>()
        .map(|s| s.root())
        .ok_or("no AssetServer")?;
    let path = root.join(&file);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let skinned = gltf::Gltf::from_slice(&bytes)
        .map(|g| g.document.skins().len() > 0)
        .unwrap_or(false);
    let rig = if skinned {
        match register(world, &id, &bytes, &path, scale) {
            Ok(rig) => Some(Arc::new(rig)),
            Err(e) => {
                // Still drawn, as its node tree in the bind pose.
                engine::core::log::warn!("{file} can't play as a rig: {e}");
                None
            }
        }
    } else {
        None
    };
    world
        .get_resource_or_init::<Rigs>()
        .loaded
        .insert(id, rig.clone());
    Ok(rig)
}

/// Import a skinned file and put its pieces in the asset stores and the animation library.
fn register(
    world: &mut World,
    id: &str,
    bytes: &[u8],
    path: &Path,
    scale: f64,
) -> Result<Rig, String> {
    let model = import_gltf(
        bytes,
        path.parent(),
        &ImportOptions {
            name: id.into(),
            scale: scale as f32,
            // As authored: the rig faces the way the still model and the DCC tool show it.
            yaw_deg: 0.0,
            ..Default::default()
        },
    )?;
    let images: Vec<Handle<Image>> = {
        let mut store = world.get_resource_or_init::<Assets<Image>>();
        model.images.into_iter().map(|i| store.add(i)).collect()
    };
    let surfaces = model
        .materials
        .iter()
        .map(|m| m.to_standard(&images))
        .collect();
    let bounds = model.mesh.bounds();
    let bones = model.skeleton.len();
    let unit_scale = model.unit_scale;
    let mesh = world
        .get_resource_or_init::<Assets<SkinnedMesh>>()
        .add(model.mesh);
    let mut library = world.get_resource_or_init::<AnimationLibrary>();
    library.add_skeleton(model.skeleton);
    let mut clips = Vec::new();
    let mut speeds = Vec::new();
    for clip in model.clips {
        let shown = clip.name.clone();
        let graph = format!("{id}#{shown}");
        speeds.push((graph.clone(), add_clip(&mut library, clip, &graph)));
        clips.push((shown, graph));
    }
    world.get_resource_or_init::<Rigs>().travel.extend(speeds);
    Ok(Rig {
        skeleton: id.into(),
        mesh,
        surfaces,
        clips,
        bounds: (bounds.min, bounds.max),
        bones,
        unit_scale,
    })
}

/// Whether `key` is an animation-only file (clips, nothing to draw).
pub fn is_clip_file(world: &mut World, key: &str) -> bool {
    let file = engine::assets::normalize_path(key);
    if let Some(&known) = world
        .get_resource::<Rigs>()
        .and_then(|r| r.clip_only.get(&file))
    {
        return known;
    }
    let root = world
        .get_resource::<AssetServer>()
        .map(|s| s.root())
        .unwrap_or_default();
    let clip_only = gltf::Gltf::open(root.join(&file))
        .map(|g| g.document.animations().len() > 0 && g.document.meshes().len() == 0)
        .unwrap_or(false);
    world
        .get_resource_or_init::<Rigs>()
        .clip_only
        .insert(file, clip_only);
    clip_only
}

/// The clips of the animation file `key`, read onto `rig`'s skeleton and put in the library.
fn clip_file(world: &mut World, rig: &Rig, key: &str) -> Result<Vec<(String, String)>, String> {
    let file = engine::assets::normalize_path(key);
    let id = format!("{}|{file}", rig.skeleton);
    if let Some(found) = world
        .get_resource::<Rigs>()
        .and_then(|r| r.clip_files.get(&id))
    {
        return Ok(found.clone());
    }
    let skeleton = world
        .get_resource::<AnimationLibrary>()
        .and_then(|l| l.skeletons.get(&rig.skeleton).cloned())
        .ok_or("the rig's skeleton isn't loaded")?;
    let root = world
        .get_resource::<AssetServer>()
        .map(|s| s.root())
        .ok_or("no AssetServer")?;
    let path = root.join(&file);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let options = ImportOptions {
        yaw_deg: 0.0,
        ..Default::default()
    };
    let (found, _) = import_clips(&bytes, path.parent(), &skeleton, rig.unit_scale, &options)?;
    let stem = crate::models::display_name(key);
    let many = found.len() > 1;
    let mut library = world.get_resource_or_init::<AnimationLibrary>();
    let mut clips = Vec::new();
    let mut speeds = Vec::new();
    for clip in found {
        // Exporters name takes "Take 001" or "mixamo.com"; the file name says more.
        let shown = if many {
            format!("{stem}: {}", clip.name)
        } else {
            stem.clone()
        };
        let graph = format!("{id}#{}", clip.name);
        speeds.push((graph.clone(), add_clip(&mut library, clip, &graph)));
        clips.push((shown, graph));
    }
    let mut rigs = world.get_resource_or_init::<Rigs>();
    rigs.travel.extend(speeds);
    rigs.clip_files.insert(id, clips.clone());
    Ok(clips)
}

/// Every clip `entity`'s rig can play: its file's own, then those of its clip files (their
/// in-place copies when it plays in place).
pub fn clips(world: &mut World, rig: &Rig, entity: Entity) -> Vec<(String, String)> {
    let mut all = rig.clips.clone();
    let options = world.get::<ModelClips>(entity).cloned().unwrap_or_default();
    for file in &options.files {
        match clip_file(world, rig, file) {
            Ok(found) => all.extend(found),
            Err(e) => engine::core::log::warn!("{file}: {e}"),
        }
    }
    if options.in_place {
        for (_, graph) in &mut all {
            graph.push_str(IN_PLACE);
        }
    }
    all
}

/// Where `entity`'s clip is and how long it is, seconds.
fn playhead(world: &World, entity: Entity) -> Option<(f32, f32)> {
    let settings = world.get::<AnimatorSettings>(entity)?;
    let duration = world
        .get_resource::<AnimationLibrary>()?
        .clip(&settings.graph)?
        .duration;
    let time = world
        .get::<Animator>(entity)
        .and_then(|a| a.graph())
        .and_then(|g| g.describe()["clips"][0]["time"].as_f64())
        .unwrap_or(0.0) as f32;
    Some((time, duration))
}

/// Put `entity`'s clip at `secs` (a fresh animator run up to that time).
fn seek(world: &mut World, entity: Entity, secs: f32) {
    let Some(settings) = world.get::<AnimatorSettings>(entity).cloned() else {
        return;
    };
    let Some(library) = world.get_resource::<AnimationLibrary>() else {
        return;
    };
    if let Ok(mut animator) = library.instantiate(&settings.skeleton, &settings.graph) {
        // A zero run would leave the pose unevaluated.
        animator.preroll(secs.max(1e-4));
        world.entity_mut(entity).insert(animator);
    }
}

/// Add the clips of the animation file `key` to the rig at `entity` and play the first, as
/// one undo step.
pub fn add_clip_file(world: &mut World, editor: &mut Editor, entity: Entity, key: &str) {
    let t = editor::now(world);
    let Some(model) = world.get::<Model>(entity).map(|m| m.path.clone()) else {
        return;
    };
    let scale = crate::models::import_scale(world, &model);
    let Ok(Some(rig)) = rig(world, &model, scale) else {
        return;
    };
    let found = match clip_file(world, &rig, key) {
        Ok(found) if !found.is_empty() => found,
        Ok(_) => {
            editor.error(t, format!("{key} has no clips"));
            return;
        }
        Err(e) => {
            editor.error(t, format!("Can't play {key} on {model}: {e}"));
            return;
        }
    };
    let before_clips = world.get::<ModelClips>(entity).cloned();
    let mut after_clips = before_clips.clone().unwrap_or_default();
    let file = engine::assets::normalize_path(key);
    if !after_clips.files.contains(&file) {
        after_clips.files.push(file);
    }
    let before = world.get::<AnimatorSettings>(entity).cloned();
    let mut after = before.clone().unwrap_or_default();
    after.graph = found[0].1.clone();
    let name = crate::models::display_name(key);
    editor::apply(
        world,
        editor,
        Command::Group(
            format!("Play {name}"),
            vec![
                Command::Component {
                    entity,
                    name: "ModelClips".into(),
                    before: before_clips.and_then(|c| serde_json::to_value(c).ok()),
                    after: serde_json::to_value(&after_clips).ok(),
                },
                Command::Component {
                    entity,
                    name: "AnimatorSettings".into(),
                    before: before.and_then(|s| serde_json::to_value(s).ok()),
                    after: serde_json::to_value(&after).ok(),
                },
            ],
        ),
    );
    editor.info(
        t,
        format!("{} plays {name}", crate::models::display_name(&model)),
    );
}

/// The rig root under the pointer when an animation file is dragged there.
pub fn clip_target(world: &mut World, key: &str, origin: DVec3, dir: Vec3) -> Option<Entity> {
    if !is_clip_file(world, key) {
        return None;
    }
    let (hit, _) = crate::picking::pick_mesh(world, origin, dir)?;
    let root = editor::selectable(world, hit);
    world
        .get::<Rigged>(root)
        .is_some_and(|r| r.animated)
        .then_some(root)
}

/// The components a rig's root carries, keeping `settings`' clip, speed and pause when that
/// clip is still in the file.
fn components(
    rig: &Rig,
    clips: &[(String, String)],
    settings: Option<&AnimatorSettings>,
    renderer: Option<&SkinnedMeshRenderer>,
) -> (SkinnedMeshRenderer, AnimatorSettings) {
    let base = renderer.cloned().unwrap_or_default();
    let mut animator = settings.cloned().unwrap_or(AnimatorSettings {
        // The editor's camera isn't a simulation focus: animate at full rate everywhere.
        lod_distance: 0.0,
        ..Default::default()
    });
    animator.skeleton = rig.skeleton.clone();
    // The same clip by its graph's name, which outlives a change of import scale.
    let clip = clip_name(&animator.graph);
    let same = |graph: &String| clip_name(graph) == clip;
    animator.graph = clips
        .iter()
        .find(|(_, graph)| same(graph))
        .or(clips.first())
        .map(|(_, graph)| graph.clone())
        .unwrap_or_default();
    (
        SkinnedMeshRenderer {
            mesh: Some(rig.mesh),
            surfaces: rig.surfaces.clone(),
            ..base
        },
        animator,
    )
}

/// Give `entity` the rig's mesh and an animator, keeping its clip choice (and its
/// [`ModelClips`]' clips) where it can.
pub fn dress(world: &mut World, rig: &Rig, entity: Entity) {
    let clips = clips(world, rig, entity);
    let Ok(mut e) = world.get_entity_mut(entity) else {
        return;
    };
    let parts = components(
        rig,
        &clips,
        e.get::<AnimatorSettings>(),
        e.get::<SkinnedMeshRenderer>(),
    );
    // A fresh animator: the library may hold a new skeleton under the same name.
    e.remove::<Animator>();
    e.insert(parts);
}

/// A clip graph's name without its rig's import scale or in-place suffix, so a rescaled rig
/// or a switch to in place finds the same clip: "imported/horse/Horse.glb@1.4#Armature|Walk"
/// → "#Armature|Walk", and a clip file's "…@1|fetched/anim/clips/m_walk.glb#m_walk" →
/// "|fetched/anim/clips/m_walk.glb#m_walk".
fn clip_name(graph: &str) -> Option<&str> {
    let (_, scaled) = graph.split_once('@')?;
    let start = scaled.find(['#', '|'])?;
    let clip = &scaled[start..];
    Some(clip.strip_suffix(IN_PLACE).unwrap_or(clip))
}

/// Graph name suffix of a clip's in-place copy.
const IN_PLACE: &str = "~in place";

/// Put a clip in the library as authored under `graph`, and a copy with its root's travel taken
/// out under `graph` + [`IN_PLACE`] (Unity's "Bake Into Pose" for root position: the
/// character walks on the spot, its sway kept), each with a one-clip graph. Returns the
/// travel speed the copy lost, m/s.
fn add_clip(library: &mut AnimationLibrary, mut clip: AnimationClip, graph: &str) -> f32 {
    let mut still = clip.clone();
    let speed = take_out_travel(&mut still);
    clip.name = graph.into();
    still.name = format!("{graph}{IN_PLACE}");
    for c in [clip, still] {
        let name = c.name.clone();
        library.add_clip(c);
        library.add_graph(AnimGraph {
            name: name.clone(),
            root: NodeDef::clip(&name),
        });
    }
    speed
}

/// Take the net travel of the clip's moving root out of it, spread evenly over the clip (the
/// track whose bone ends farthest from where it starts: the hips of a walk, whatever the
/// rig calls them). Returns the speed removed, m/s.
fn take_out_travel(clip: &mut AnimationClip) -> f32 {
    let travel = |t: &engine_anim::clip::BoneTrack| match (
        t.translation.values.first(),
        t.translation.values.last(),
    ) {
        (Some(a), Some(b)) => *b - *a,
        _ => Vec3::ZERO,
    };
    let Some(track) = clip
        .tracks
        .iter_mut()
        .max_by(|a, b| travel(a).length().total_cmp(&travel(b).length()))
    else {
        return 0.0;
    };
    let moved = travel(track);
    // Under a centimetre: an idle's drift, not a walk.
    if moved.length() < 0.01 || clip.duration <= 0.0 {
        return 0.0;
    }
    let (first, last) = (
        track.translation.times.first().copied().unwrap_or(0.0),
        track.translation.times.last().copied().unwrap_or(0.0),
    );
    let span = (last - first).max(1e-6);
    for (t, v) in track
        .translation
        .times
        .iter()
        .zip(track.translation.values.iter_mut())
    {
        *v -= moved * ((t - first) / span);
    }
    moved.length() / clip.duration
}

/// Give placed models that came from a scene file or an undo their rig again (Update,
/// exclusive). Roots saved as rigs carry `AnimatorSettings`; older scenes saved the still node
/// tree, which stays as it is until the model is reimported.
pub fn sync(world: &mut World) {
    let mut q = world.query_filtered::<(Entity, &Model, Has<AnimatorSettings>), (With<InScene>, Without<Rigged>)>();
    let todo: Vec<(Entity, String, bool)> = q
        .iter(world)
        .map(|(e, m, a)| (e, m.path.clone(), a))
        .collect();
    for (entity, key, saved_as_rig) in todo {
        let rig = if saved_as_rig {
            let scale = crate::models::import_scale(world, &key);
            match rig(world, &key, scale) {
                Ok(rig) => rig,
                Err(e) => {
                    engine::core::log::warn!("{key}: {e}");
                    None
                }
            }
        } else {
            None
        };
        if let Some(rig) = &rig {
            dress(world, rig, entity);
        }
        let Ok(mut e) = world.get_entity_mut(entity) else {
            continue;
        };
        e.insert(Rigged {
            animated: rig.is_some(),
            preview: true,
        });
    }
}

/// Play previewing rigs' clips while the world holds still (Update, before the animators).
pub fn preview(
    time: Res<Time>,
    editor: Res<Editor>,
    mut q: Query<(&Rigged, &AnimatorSettings, &mut Animator)>,
) {
    if editor.mode != Mode::Edit {
        return;
    }
    let dt = time.real_delta_secs().min(0.1);
    for (rigged, settings, mut animator) in &mut q {
        if rigged.preview && !settings.paused {
            animator.preroll(dt * settings.speed);
        }
    }
}

/// The Details section of a rigged model: clip, speed, pause, in place, and a timeline to play,
/// pause and scrub the clip while editing (Unity's preview and Unreal Persona's transport).
pub fn animation_panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, entity: Entity) {
    let Some(key) = world.get::<Model>(entity).map(|m| m.path.clone()) else {
        return;
    };
    if !world.get::<Rigged>(entity).is_some_and(|r| r.animated) {
        return;
    }
    let Some(settings) = world.get::<AnimatorSettings>(entity).cloned() else {
        return;
    };
    let scale = crate::models::import_scale(world, &key);
    let Ok(Some(rig)) = rig(world, &key, scale) else {
        return;
    };
    let clips = clips(world, &rig, entity);
    egui::CollapsingHeader::new("🎬 Animation")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(format!(
                "{} bones, {} clips ({} from other files)",
                rig.bones,
                clips.len(),
                clips.len() - rig.clips.len()
            ))
            .on_hover_text(
                "Drag an animation file from Content onto the character to add its clips \
                 (bones are matched by name).",
            );
            let mut after = settings.clone();
            let current = clips
                .iter()
                .find(|(_, graph)| *graph == settings.graph)
                .map_or("(rest pose)", |(shown, _)| shown.as_str())
                .to_string();
            egui::ComboBox::from_label("Clip")
                .selected_text(&current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut after.graph, String::new(), "(rest pose)");
                    for (shown, graph) in &clips {
                        ui.selectable_value(&mut after.graph, graph.clone(), shown);
                    }
                });
            ui.horizontal(|ui| {
                ui.label("Speed");
                ui.add(
                    egui::DragValue::new(&mut after.speed)
                        .speed(0.01)
                        .range(0.0..=4.0),
                );
                ui.checkbox(&mut after.paused, "Paused")
                    .on_hover_text("Hold the pose in Play too.");
            });
            in_place_toggle(ui, world, editor, entity, &clips, &settings);
            timeline(ui, world, entity, editor.mode == Mode::Edit);
            if after != settings {
                editor::apply(
                    world,
                    editor,
                    Command::Component {
                        entity,
                        name: "AnimatorSettings".into(),
                        before: serde_json::to_value(&settings).ok(),
                        after: serde_json::to_value(&after).ok(),
                    },
                );
            }
        });
}

/// "In place" for every clip, switching the playing clip to its in-place copy (or back) in the
/// same undo step.
fn in_place_toggle(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    entity: Entity,
    clips: &[(String, String)],
    settings: &AnimatorSettings,
) {
    let mut options = world.get::<ModelClips>(entity).cloned().unwrap_or_default();
    let speed = world
        .get_resource::<Rigs>()
        .and_then(|r| r.travel.get(settings.graph.trim_end_matches(IN_PLACE)))
        .copied()
        .unwrap_or(0.0);
    let hint = if speed > 0.0 {
        format!("in place ({speed:.2} m/s of travel taken out)")
    } else {
        "in place".to_string()
    };
    if ui
        .checkbox(&mut options.in_place, hint)
        .on_hover_text(
            "Take each clip's root travel out so walks and runs play on the spot, sway kept \
             (Unity: Root Transform Position, Bake Into Pose). Games then move the character.",
        )
        .changed()
    {
        set_in_place(world, editor, entity, clips, settings, options);
    }
}

/// Apply `options` (with a changed `in_place`) and move the playing clip to its in-place copy
/// or back, as one undo step.
fn set_in_place(
    world: &mut World,
    editor: &mut Editor,
    entity: Entity,
    clips: &[(String, String)],
    settings: &AnimatorSettings,
    options: ModelClips,
) {
    let before = world.get::<ModelClips>(entity).cloned();
    let mut after = settings.clone();
    if let Some((_, graph)) = clips.iter().find(|(_, g)| *g == settings.graph) {
        let base = graph.trim_end_matches(IN_PLACE).to_string();
        after.graph = if options.in_place {
            format!("{base}{IN_PLACE}")
        } else {
            base
        };
    }
    editor::apply(
        world,
        editor,
        Command::Group(
            if options.in_place {
                "Play in place".into()
            } else {
                "Play with travel".into()
            },
            vec![
                Command::Component {
                    entity,
                    name: "ModelClips".into(),
                    before: before.and_then(|c| serde_json::to_value(c).ok()),
                    after: serde_json::to_value(&options).ok(),
                },
                Command::Component {
                    entity,
                    name: "AnimatorSettings".into(),
                    before: serde_json::to_value(settings).ok(),
                    after: serde_json::to_value(&after).ok(),
                },
            ],
        ),
    );
}

/// Play/pause, back to the start, one frame on, and a scrub bar with the time and frame.
fn timeline(ui: &mut egui::Ui, world: &mut World, entity: Entity, editing: bool) {
    const FPS: f32 = 30.0;
    let Some((time, duration)) = playhead(world, entity) else {
        return;
    };
    let playing = world.get::<Rigged>(entity).is_some_and(|r| r.preview);
    let mut seek_to = None;
    let mut play = None;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(editing, |ui| {
            let (icon, tip) = if playing {
                ("⏸", "Pause the preview")
            } else {
                ("▶", "Play the clip while editing")
            };
            if ui.button(icon).on_hover_text(tip).clicked() {
                play = Some(!playing);
            }
            if ui.button("⏮").on_hover_text("Back to the start").clicked() {
                seek_to = Some(0.0);
            }
            if ui.button("⏭").on_hover_text("One frame on").clicked() {
                seek_to = Some((time + 1.0 / FPS).min(duration));
            }
            let mut t = time;
            ui.spacing_mut().slider_width = (ui.available_width() - 4.0).max(40.0);
            if ui
                .add(egui::Slider::new(&mut t, 0.0..=duration.max(1e-3)).show_value(false))
                .on_hover_text("Drag to any moment of the clip")
                .changed()
            {
                seek_to = Some(t);
            }
        });
    });
    ui.label(
        egui::RichText::new(format!(
            "{time:.2} / {duration:.2} s   frame {} / {}",
            (time * FPS).round() as i32,
            (duration * FPS).round() as i32
        ))
        .monospace()
        .small(),
    );
    if let Some(on) = play
        && let Some(mut rigged) = world.get_mut::<Rigged>(entity)
    {
        rigged.preview = on;
    }
    if let Some(t) = seek_to {
        scrub(world, entity, t);
    }
}

/// Play `entity`'s clips in place or with their travel (scripts and tests; Details has the
/// checkbox).
pub fn play_in_place(world: &mut World, editor: &mut Editor, entity: Entity, on: bool) {
    let Some(key) = world.get::<Model>(entity).map(|m| m.path.clone()) else {
        return;
    };
    let scale = crate::models::import_scale(world, &key);
    let (Ok(Some(rig)), Some(settings)) = (
        rig(world, &key, scale),
        world.get::<AnimatorSettings>(entity).cloned(),
    ) else {
        return;
    };
    let clips = clips(world, &rig, entity);
    let mut options = world.get::<ModelClips>(entity).cloned().unwrap_or_default();
    options.in_place = on;
    set_in_place(world, editor, entity, &clips, &settings, options);
}

/// Hold `entity`'s clip at `secs` (stops the edit-mode preview there).
pub fn scrub(world: &mut World, entity: Entity, secs: f32) {
    if let Some(mut rigged) = world.get_mut::<Rigged>(entity) {
        rigged.preview = false;
    }
    seek(world, entity, secs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_names_ignore_the_import_scale() {
        assert_eq!(
            clip_name("imported/horse/Horse.glb@1.4#Armature|Walk"),
            Some("#Armature|Walk")
        );
        assert_eq!(
            clip_name("a/walker.glb@1|fetched/anim/clips/m_walk.glb#m_walk"),
            Some("|fetched/anim/clips/m_walk.glb#m_walk")
        );
        assert_eq!(clip_name(""), None);
    }

    #[test]
    fn in_place_takes_the_walk_out_and_keeps_the_sway() {
        use engine_anim::clip::{BoneTrack, Track};
        let mut clip = AnimationClip::new("walk", 2.0);
        let mut hips = BoneTrack {
            bone: "Hips".into(),
            ..Default::default()
        };
        hips.translation = Track {
            times: vec![0.0, 1.0, 2.0],
            values: vec![
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.1, 1.05, 1.5),
                Vec3::new(0.0, 1.0, 3.0),
            ],
            step: false,
        };
        clip.tracks.push(hips);
        let speed = take_out_travel(&mut clip);
        assert!((speed - 1.5).abs() < 1e-5, "3 m in 2 s: {speed}");
        let v = &clip.tracks[0].translation.values;
        assert_eq!(v[0], v[2], "loops on the spot");
        assert!(
            (v[1] - Vec3::new(0.1, 1.05, 0.0)).length() < 1e-5,
            "sway kept: {}",
            v[1]
        );

        let mut idle = AnimationClip::new("idle", 2.0);
        assert_eq!(take_out_travel(&mut idle), 0.0);
        assert_eq!(clip_name("a.glb@1#walk~in place"), Some("#walk"));
    }

    #[test]
    fn a_skinned_file_places_as_a_rig_and_comes_back_after_undo() {
        use engine::render::{GltfScene, Mesh};
        let dir = std::env::temp_dir().join(format!("editor-rigs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fbx = dir.join("source.fbx");
        std::fs::write(&fbx, crate::convert::tests::BENDING_BAR).unwrap();
        crate::convert::to_glb(&fbx, &dir.join("bar.glb")).unwrap();
        let mut app = App::new();
        app.add_plugins(engine::assets::AssetPlugin {
            hot_reload: false,
            root: Some(dir.clone()),
        });
        app.init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .init_asset::<GltfScene>()
            .init_asset::<SkinnedMesh>()
            .init_resource::<AnimationLibrary>()
            .register_asset_loader(engine::render::GltfLoader)
            .register_component::<Model>("test")
            .register_component::<AnimatorSettings>("test")
            .register_component::<ModelClips>("test");
        app.update();
        let world = &mut app.world;
        let camera = world.spawn(crate::scene_ops::EditorOnly).id();
        let mut editor = Editor::new(camera);

        let placed =
            crate::models::place(world, &mut editor, "bar.glb", DVec3::new(1.0, 0.0, 0.0)).unwrap();
        let check = |world: &World, e: Entity| {
            let graph = &world.get::<AnimatorSettings>(e).unwrap().graph;
            assert!(graph.ends_with("#Bend"), "plays its take: {graph}");
            assert!(world.get::<SkinnedMeshRenderer>(e).unwrap().mesh.is_some());
            assert!(world.get::<Children>(e).is_none(), "no still node tree");
        };
        check(world, placed);
        assert!(world.get::<Rigged>(placed).unwrap().animated);

        // Undo and redo respawn it from its saved components; the rig is put back from the file.
        editor::undo(world, &mut editor);
        editor::redo(world, &mut editor);
        let mut q = world.query_filtered::<Entity, With<Model>>();
        let again = q.single(world).unwrap();
        sync(world);
        check(world, again);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
