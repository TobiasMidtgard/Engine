//! Scripted headless runs that drive the editor like a person would and save screenshots of
//! the whole window: `editor --headless --size 1600x900 --shots <dir>`. Used for the gallery
//! (Tobias reviews the editor from these images) and as an end-to-end smoke test.

use std::collections::VecDeque;
use std::path::PathBuf;

use engine::prelude::*;

use crate::editor::{self, Create, Editor, Tab, Tool};
use crate::landscape::SculptOp;
use crate::ui::EditorDock;

/// One step of a script.
pub enum Step {
    /// Let this many frames render (egui lays out, TAA settles).
    Wait(u32),
    /// Save the window to `<dir>/<name>.png`.
    Shot(&'static str),
    /// Select the scene entity with this name (empty: nothing).
    Select(&'static str),
    Tool(Tool),
    /// Put the editor camera at a point looking at another.
    Camera(DVec3, DVec3),
    /// Bring a dock tab to the front.
    Tab(Tab),
    /// Anything else.
    Do(Box<Action>),
}

/// A scripted step that does anything with the world and the editor.
pub type Action = dyn Fn(&mut World, &mut Editor) + Send + Sync;

/// The running script.
#[derive(Resource)]
pub struct ShotScript {
    steps: VecDeque<Step>,
    dir: PathBuf,
    wait: u32,
}

impl ShotScript {
    pub fn new(dir: impl Into<PathBuf>, steps: Vec<Step>) -> Self {
        Self {
            steps: steps.into(),
            dir: dir.into(),
            wait: 0,
        }
    }
}

/// Advance the script by one step per frame (Update, exclusive, after the UI).
pub fn run_script(world: &mut World) {
    let Some(mut script) = world.remove_resource::<ShotScript>() else {
        return;
    };
    if script.wait > 0 {
        script.wait -= 1;
        world.insert_resource(script);
        return;
    }
    let Some(step) = script.steps.pop_front() else {
        engine::core::log::info!("shot script finished");
        world.write_message(AppExit::Success);
        return;
    };
    world.resource_scope(|world, mut editor: Mut<Editor>| {
        let editor = &mut *editor;
        match step {
            Step::Wait(n) => script.wait = n,
            Step::Shot(name) => {
                let path = script.dir.join(format!("{name}.png"));
                engine::core::log::info!("shot {}", path.display());
                world
                    .resource_mut::<engine::render::Screenshots>()
                    .request(path.to_string_lossy().to_string());
                // The screenshot is read back at the end of this frame's render.
                script.wait = 1;
            }
            Step::Select(name) => {
                let found = find(world, name);
                editor.select(found);
                if found.is_none() && !name.is_empty() {
                    editor.error(editor::now(world), format!("script: no entity '{name}'"));
                }
            }
            Step::Tool(t) => editor.tool = t,
            Step::Camera(at, target) => {
                if let Some(mut t) = world.get_mut::<Transform>(editor.camera) {
                    *t = Transform::from_translation(at).looking_at(target, Vec3::Y);
                    let fwd = t.forward();
                    if let Some(mut cam) =
                        world.get_mut::<crate::camera::EditorCamera>(editor.camera)
                    {
                        *cam = crate::camera::EditorCamera::looking(fwd);
                    }
                }
            }
            Step::Tab(tab) => {
                let mut dock = world.resource_mut::<EditorDock>();
                if let Some(path) = dock.0.find_tab(&tab) {
                    let _ = dock.0.set_active_tab(path);
                }
            }
            Step::Do(f) => f(world, editor),
        }
    });
    world.insert_resource(script);
}

/// The scene entity with this name.
pub fn find(world: &mut World, name: &str) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Name)>();
    q.iter(world)
        .filter(|(e, n)| n.as_str() == name && world.get::<crate::scene_ops::InScene>(*e).is_some())
        .map(|(e, _)| e)
        .next()
}

fn translate(world: &mut World, editor: &mut Editor, name: &str, by: DVec3) {
    let Some(e) = find(world, name) else { return };
    let Some(before) = world.get::<Transform>(e).copied() else {
        return;
    };
    let mut after = before;
    after.translation += by;
    editor::apply(
        world,
        editor,
        crate::undo::Command::Component {
            entity: e,
            name: "Transform".into(),
            before: serde_json::to_value(before).ok(),
            after: serde_json::to_value(after).ok(),
        },
    );
    editor.history.interaction += 1;
}

/// The gallery tour: overview, selection with each gizmo, the inspector on a light, editing
/// with undo, a module demo, play and stop, and saving.
pub fn gallery(dir: impl Into<PathBuf>) -> ShotScript {
    let overview_cam = (DVec3::new(9.5, 6.0, 9.0), DVec3::new(-0.5, 0.6, -2.0));
    let steps = vec![
        Step::Camera(overview_cam.0, overview_cam.1),
        Step::Wait(6),
        Step::Shot("01_editor_overview"),
        Step::Select("Crate 6"),
        Step::Tool(Tool::Move),
        Step::Wait(3),
        Step::Shot("02_select_move_gizmo"),
        Step::Select("Barrel"),
        Step::Tool(Tool::Rotate),
        Step::Wait(3),
        Step::Shot("03_rotate_gizmo_and_details"),
        Step::Select("Lamp"),
        Step::Tool(Tool::Scale),
        Step::Do(Box::new(|world, editor| {
            // Unfold the lamp post in the outliner by selecting through it.
            let _ = world;
            editor.local_space = true;
        })),
        Step::Wait(3),
        Step::Shot("04_light_inspector_scale_gizmo"),
        // Edit, then undo.
        Step::Select("Boulder"),
        Step::Tool(Tool::Move),
        Step::Do(Box::new(|world, editor| {
            translate(world, editor, "Boulder", DVec3::new(2.5, 0.0, 1.0));
        })),
        Step::Tab(Tab::History),
        Step::Wait(3),
        Step::Shot("05_moved_boulder_history"),
        Step::Do(Box::new(editor::undo)),
        Step::Wait(3),
        Step::Shot("06_after_undo"),
        // A module demo placed as one object.
        Step::Do(Box::new(|world, editor| {
            editor::create(
                world,
                editor,
                Create::Demo("physics"),
                DVec3::new(0.0, 0.0, 26.0),
            );
        })),
        Step::Tab(Tab::Content),
        Step::Camera(DVec3::new(18.0, 14.0, 48.0), DVec3::new(0.0, 1.0, 26.0)),
        Step::Wait(8),
        Step::Shot("07_physics_demo_placed"),
        // Play: crates and the ball are physics bodies.
        Step::Select(""),
        Step::Camera(overview_cam.0, overview_cam.1),
        Step::Do(Box::new(|world, editor| {
            translate(world, editor, "Crate 6", DVec3::new(0.0, 1.5, 0.0));
            editor::play(world, editor);
        })),
        Step::Tab(Tab::Console),
        Step::Wait(40),
        Step::Shot("08_playing"),
        Step::Do(Box::new(editor::stop)),
        Step::Wait(4),
        Step::Shot("09_stopped_restored"),
        Step::Do(Box::new(|world, editor| {
            editor::save_scene(world, editor, "scenes/courtyard.scene.json");
        })),
        Step::Select("Crate stack"),
        Step::Tab(Tab::Content),
        Step::Wait(3),
        Step::Shot("10_saved_scene"),
    ];
    ShotScript::new(dir, steps)
}

/// Round 2's features: box select, copy and paste, prefabs.
pub fn round2(dir: impl Into<PathBuf>) -> ShotScript {
    let overview_cam = (DVec3::new(9.5, 6.0, 9.0), DVec3::new(-0.5, 0.6, -2.0));
    let (from, to) = (egui::pos2(690.0, 290.0), egui::pos2(1000.0, 480.0));
    let steps = vec![
        Step::Camera(overview_cam.0, overview_cam.1),
        Step::Wait(6),
        // Box select: the box as it is dragged, then what it selected.
        Step::Do(Box::new(move |world, editor| {
            editor.box_start = Some(from);
            world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected
                .push(egui::Event::PointerMoved(to));
        })),
        Step::Wait(2),
        Step::Shot("11_box_select_drag"),
        Step::Do(Box::new(move |world, editor| {
            editor.box_start = None;
            if let Some(view) = editor.view {
                let area = egui::Rect::from_two_pos(from, to);
                editor.selection = crate::viewport::entities_in_box(world, &view, area);
            }
        })),
        Step::Wait(3),
        Step::Shot("12_box_selected"),
        // Copy and paste the barrel, then move the copy aside.
        Step::Select("Barrel"),
        Step::Do(Box::new(|world, editor| {
            editor::copy_selection(world, editor);
            editor::paste(world, editor);
            move_selection(world, editor, DVec3::new(-1.3, 0.0, 1.0));
        })),
        Step::Tab(Tab::History),
        Step::Wait(3),
        Step::Shot("13_copy_paste"),
        // Prefabs: save the lamp post, place two more, change one, apply to all.
        Step::Do(Box::new(|world, editor| {
            if let Some(post) = find(world, "Lamp post") {
                crate::prefab::create(world, editor, post);
                let path = crate::prefab::path_for("Lamp post");
                for at in [DVec3::new(-3.0, 0.0, 2.5), DVec3::new(0.5, 0.0, 4.0)] {
                    crate::prefab::place(world, editor, &path, None, None);
                    move_selection_to(world, editor, at);
                }
            }
        })),
        Step::Select(""),
        Step::Tab(Tab::Content),
        Step::Wait(4),
        Step::Shot("14_prefab_instances"),
        Step::Do(Box::new(|world, editor| {
            // The original post (still where the starter scene put it): a blue, bigger lamp.
            let mut q = world.query::<(Entity, &Name, &Transform, &Children)>();
            let original = q
                .iter(world)
                .find(|(_, n, t, _)| {
                    n.as_str() == "Lamp post" && (t.translation.x - 2.5).abs() < 0.01
                })
                .map(|(e, _, _, c)| (e, c.iter().next()));
            let Some((post, Some(lamp))) = original else {
                return;
            };
            if let Some(mut light) = world.get_mut::<PointLight>(lamp) {
                light.color = [0.35, 0.6, 1.0];
            }
            if let Some(mut t) = world.get_mut::<Transform>(lamp) {
                t.scale = Vec3::splat(1.8);
            }
            editor.select(Some(post));
            crate::prefab::apply_all(world, editor, post);
        })),
        Step::Tab(Tab::History),
        Step::Wait(4),
        Step::Shot("15_prefab_applied"),
    ];
    ShotScript::new(dir, steps)
}

/// Move every selected entity by `by` (one undo step per entity is fine for scripts).
fn move_selection(world: &mut World, editor: &mut Editor, by: DVec3) {
    for e in editor.selection.clone() {
        if let Some(mut t) = world.get_mut::<Transform>(e) {
            t.translation += by;
        }
    }
}

/// Put the selected entities at `at` across the ground, keeping their height.
fn move_selection_to(world: &mut World, editor: &mut Editor, at: DVec3) {
    for e in editor.selection.clone() {
        if let Some(mut t) = world.get_mut::<Transform>(e) {
            t.translation.x = at.x;
            t.translation.z = at.z;
        }
    }
}

/// Round 3: a landscape, sculpted and painted.
pub fn round3(dir: impl Into<PathBuf>) -> ShotScript {
    let cam = (DVec3::new(42.0, 16.0, -46.0), DVec3::new(-8.0, 2.0, -6.0));
    let steps = vec![
        Step::Camera(cam.0, cam.1),
        Step::Do(Box::new(crate::landscape::create)),
        Step::Select(""),
        Step::Wait(14),
        Step::Shot("16_landscape_created"),
        // A hill behind the courtyard.
        Step::Do(Box::new(|world, editor| {
            // Dabs in one frame all see the ground as it was, so each one starts where the
            // last one left off (the live brush gets that from the ground itself).
            for k in 0..14 {
                let at = DVec3::new(-22.0, 0.8 * k as f64, -24.0);
                sculpt(world, editor, at, 14.0, 0.8, SculptOp::Raise);
            }
        })),
        // Far enough apart in time to be separate undo steps.
        Step::Wait(16),
        // A trench down the right side.
        Step::Do(Box::new(|world, editor| {
            for pass in 0..4 {
                for i in 0..=24 {
                    let t = i as f64 / 24.0;
                    let at = DVec3::new(16.0 + 6.0 * t, -0.6 * pass as f64, -24.0 + 40.0 * t);
                    sculpt(world, editor, at, 3.5, 0.6, SculptOp::Lower);
                }
            }
        })),
        Step::Wait(16),
        // A dirt path from the courtyard to the hill, and sand around the trench.
        Step::Do(Box::new(|world, editor| {
            for i in 0..=30 {
                let t = i as f64 / 30.0;
                let at = DVec3::new(
                    -6.0 - 14.0 * t,
                    0.0,
                    -2.0 - 18.0 * t + 6.0 * (t * 3.0).sin(),
                );
                sculpt(world, editor, at, 2.2, 1.0, SculptOp::Paint("dirt".into()));
            }
            for i in 0..=12 {
                let t = i as f64 / 12.0;
                let at = DVec3::new(16.0 + 6.0 * t, 0.0, -24.0 + 40.0 * t);
                sculpt(world, editor, at, 7.0, 1.0, SculptOp::Paint("sand".into()));
            }
        })),
        Step::Tool(Tool::Sculpt),
        Step::Do(Box::new(|world, _| {
            world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected
                .push(egui::Event::PointerMoved(egui::pos2(640.0, 520.0)));
        })),
        Step::Tab(Tab::History),
        Step::Wait(14),
        Step::Do(Box::new(|world, editor| {
            let fog = world.resource::<FogSettings>();
            editor.info(
                editor::now(world),
                format!("fog density {} at {} m", fog.density, fog.base_height),
            );
            let ground = world.resource::<engine::world::Ground>().0.clone();
            for (what, x, z) in [
                ("hill", -22.0, -24.0),
                ("trench", 19.0, -4.0),
                ("pad", 5.0, 10.0),
            ] {
                let h = ground
                    .sample(DVec3::new(x, 0.0, z))
                    .map_or(f64::NAN, |s| s.height);
                editor.info(
                    editor::now(world),
                    format!("ground at the {what}: {h:.2} m"),
                );
            }
        })),
        Step::Shot("17_sculpted_and_painted"),
        Step::Do(Box::new(editor::undo)),
        Step::Wait(14),
        Step::Shot("18_undo_paint"),
    ];
    ShotScript::new(dir, steps)
}

/// Round 4: models from the asset folder, placed by click and by drag, saved and reopened.
pub fn round4(dir: impl Into<PathBuf>) -> ShotScript {
    const HORSE: &str = "fetched/anim/animals/horse/Horse.gltf";
    const CAR: &str = "fetched/anim/vehicles/car_concept.glb";
    const WALKER: &str = "fetched/anim/characters/walker.glb";
    const SCENE: &str = "scenes/round4_models.scene.json";
    let steps = vec![
        Step::Camera(DVec3::new(9.0, 3.2, 13.0), DVec3::new(2.0, 0.8, 2.0)),
        Step::Select(""),
        Step::Tab(Tab::Content),
        Step::Do(Box::new(|world, editor| {
            crate::models::place(world, editor, CAR, DVec3::new(5.0, 0.0, 0.0));
            crate::models::place(world, editor, WALKER, DVec3::new(0.5, 0.0, 4.0));
        })),
        // Textures stream in over a few frames.
        Step::Wait(12),
        // Drag the horse in: a payload held over the scene view, then released.
        Step::Do(Box::new(|world, _| {
            let ctx = world.resource::<crate::egui_bridge::Egui>().ctx.clone();
            egui::DragAndDrop::set_payload(&ctx, crate::models::Dragged::Model(HORSE.into()));
            world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected
                .push(egui::Event::PointerMoved(egui::pos2(560.0, 470.0)));
        })),
        Step::Wait(3),
        Step::Shot("19_drag_model"),
        Step::Do(Box::new(|world, _| {
            let mut input = world.resource_mut::<crate::egui_bridge::EguiInput>();
            input.injected.push(egui::Event::PointerButton {
                pos: egui::pos2(560.0, 470.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
        })),
        Step::Wait(14),
        Step::Shot("20_models_placed"),
        // The car's file changes on disk: the editor notices and rebuilds it in place.
        Step::Do(Box::new(|world, _| {
            let root = world
                .resource::<engine::assets::AssetServer>()
                .root()
                .join(CAR);
            if let Ok(file) = std::fs::File::options().append(true).open(root) {
                let _ = file.set_modified(std::time::SystemTime::now());
            }
        })),
        Step::Wait(40),
        // A file dragged from the desktop onto the scene view: copied in and placed where it
        // lands.
        Step::Do(Box::new(|world, _| {
            let root = world.resource::<engine::assets::AssetServer>().root();
            let downloads = std::env::temp_dir().join("Downloads");
            let _ = std::fs::create_dir_all(&downloads);
            let source = downloads.join("Lifter.glb");
            let _ = std::fs::copy(root.join("fetched/anim/characters/lifter.glb"), &source);
            world.write_message(engine::input::FileDrop::Hovered(source));
            world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected
                .push(egui::Event::PointerMoved(egui::pos2(900.0, 520.0)));
        })),
        Step::Wait(3),
        Step::Shot("21_drop_file"),
        Step::Do(Box::new(|world, _| {
            let source = std::env::temp_dir().join("Downloads/Lifter.glb");
            world.write_message(engine::input::FileDrop::Dropped(source));
        })),
        Step::Wait(30),
        Step::Shot("21_imported"),
        // Save, clear, and open the scene again: the models come back from their asset keys.
        Step::Do(Box::new(|world, editor| {
            editor::save_scene(world, editor, SCENE);
            editor::new_scene(world, editor);
        })),
        Step::Wait(4),
        Step::Do(Box::new(|world, editor| {
            editor::load_scene(world, editor, SCENE);
            let mut q = world.query_filtered::<(), (
                With<crate::scene_ops::InScene>,
                With<engine::render::MeshRenderer>,
            )>();
            let meshes = q.iter(world).count();
            editor.info(
                editor::now(world),
                format!("reopened {SCENE}: {meshes} mesh entities"),
            );
        })),
        Step::Tab(Tab::Console),
        Step::Wait(14),
        Step::Shot("22_reopened"),
    ];
    ShotScript::new(dir, steps)
}

/// Round 5: FBX import beside the glTF original, import settings, Blender-style scale fix.
pub fn round5(dir: impl Into<PathBuf>) -> ShotScript {
    const CAR: &str = "fetched/anim/vehicles/car_concept.glb";
    fn drop_file(at: egui::Pos2, path: &'static str) -> Vec<Step> {
        vec![
            Step::Do(Box::new(move |world, _| {
                world
                    .resource_mut::<crate::egui_bridge::EguiInput>()
                    .injected
                    .push(egui::Event::PointerMoved(at));
            })),
            Step::Wait(2),
            Step::Do(Box::new(move |world, _| {
                let source = std::env::var("ROUND5_FILES").unwrap_or_default();
                world.write_message(engine::input::FileDrop::Dropped(
                    PathBuf::from(source).join(path),
                ));
            })),
            Step::Wait(2),
        ]
    }
    let mut steps = vec![
        Step::Camera(DVec3::new(10.0, 4.0, 14.0), DVec3::new(1.0, 0.6, 3.0)),
        Step::Select(""),
        Step::Tab(Tab::Console),
        Step::Do(Box::new(|world, editor| {
            crate::models::place(world, editor, CAR, DVec3::new(-1.0, 0.0, 6.0));
        })),
    ];
    steps.extend(drop_file(egui::pos2(900.0, 470.0), "car/Sports Car.fbx"));
    steps.extend(drop_file(egui::pos2(470.0, 560.0), "horse/Horse.fbx"));
    steps.extend([
        Step::Do(Box::new(|world, editor| {
            let mut q = world.query_filtered::<(Entity, &crate::models::Model), ()>();
            let car = q
                .iter(world)
                .find(|(_, m)| m.path.ends_with("Sports Car.glb"))
                .map(|(e, _)| e);
            editor.selection = car.into_iter().collect();
        })),
        Step::Wait(40),
        Step::Shot("23_fbx_imported"),
        // The settings panel: scale the imported horse up, apply, reimport.
        Step::Do(Box::new(|world, editor| {
            let root = world.resource::<engine::assets::AssetServer>().root();
            let key = "imported/horse/Horse.glb";
            if let Some(mut settings) = crate::import::settings_for(&root, key) {
                settings.scale = 1.4;
                let _ = crate::import::save_settings(&root, key, &settings);
            }
            crate::models::reimport(world, editor, key);
            let mut q = world.query_filtered::<(Entity, &crate::models::Model), ()>();
            editor.selection = q
                .iter(world)
                .filter(|(_, m)| m.path == key)
                .map(|(e, _)| e)
                .collect();
        })),
        Step::Wait(30),
        Step::Shot("24_import_settings"),
    ]);
    ShotScript::new(dir, steps)
}

/// Round 6: rigs. The glTF horse and the same horse converted from FBX (skin and takes) play
/// in edit mode; a clip file dragged onto a Rocketbox character makes it walk; the clip picker
/// switches the FBX horse to its trot; the scene saves and reopens with its rigs playing.
pub fn round6(dir: impl Into<PathBuf>) -> ShotScript {
    const HORSE: &str = "fetched/anim/animals/horse/Horse.gltf";
    const WALKER: &str = "fetched/anim/characters/walker.glb";
    const CLIP: &str = "fetched/anim/clips/m_walk_neutral_01.glb";
    const FBX_HORSE: &str = "imported/horse/Horse.glb";
    const SCENE: &str = "scenes/round6_rigs.scene.json";
    fn find(world: &mut World, key: &str) -> Option<Entity> {
        let mut q = world.query::<(Entity, &crate::models::Model)>();
        q.iter(world).find(|(_, m)| m.path == key).map(|(e, _)| e)
    }
    /// Switch the rig `key` to its clip `clip`, through the history like the clip picker.
    fn play(world: &mut World, editor: &mut Editor, key: &str, clip: &str) {
        let Some(rig) = find(world, key) else {
            return;
        };
        let Some(before) = world.get::<engine_anim::AnimatorSettings>(rig).cloned() else {
            return;
        };
        let mut after = before.clone();
        if let Some((id, _)) = before.graph.split_once('#') {
            after.graph = format!("{id}#{clip}");
        }
        editor::apply(
            world,
            editor,
            crate::undo::Command::Component {
                entity: rig,
                name: "AnimatorSettings".into(),
                before: serde_json::to_value(&before).ok(),
                after: serde_json::to_value(&after).ok(),
            },
        );
    }
    /// Where the walker's chest is on screen.
    fn walker_on_screen(world: &mut World, editor: &Editor) -> egui::Pos2 {
        let at = find(world, WALKER)
            .and_then(|e| world.get::<GlobalTransform>(e))
            .map_or(DVec3::ZERO, |g| g.translation());
        editor
            .view
            .and_then(|v| v.project(at + DVec3::Y * 1.1))
            .unwrap_or(egui::pos2(640.0, 400.0))
    }
    let steps = vec![
        Step::Camera(DVec3::new(0.3, 1.9, 6.2), DVec3::new(0.0, 0.8, 0.0)),
        Step::Select(""),
        Step::Tab(Tab::Console),
        Step::Do(Box::new(|world, editor| {
            crate::models::place(world, editor, HORSE, DVec3::new(-2.0, 0.0, 0.0));
            crate::models::place(world, editor, WALKER, DVec3::new(0.0, 0.0, 1.2));
            // The FBX horse comes in as a file dropped from the desktop.
            let source = std::env::var("ROUND6_FILES").unwrap_or_default();
            crate::import::import_and_place(
                world,
                editor,
                &PathBuf::from(source).join("horse/Horse.fbx"),
                Some(DVec3::new(2.0, 0.0, 0.0)),
            );
            // Both horses walk, in step: the converted one should move like the original.
            play(world, editor, FBX_HORSE, "Armature|Walk");
            editor.selection = find(world, FBX_HORSE).into_iter().collect();
        })),
        Step::Wait(40),
        Step::Shot("25_rigs_playing"),
        Step::Tab(Tab::Content),
        // Drag the walk clip from Content onto the standing character.
        Step::Do(Box::new(|world, editor| {
            let at = walker_on_screen(world, editor);
            let ctx = world.resource::<crate::egui_bridge::Egui>().ctx.clone();
            egui::DragAndDrop::set_payload(&ctx, crate::models::Dragged::Model(CLIP.into()));
            world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected
                .push(egui::Event::PointerMoved(at));
        })),
        Step::Wait(3),
        Step::Shot("26_drag_clip"),
        Step::Do(Box::new(|world, editor| {
            let at = walker_on_screen(world, editor);
            world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected
                .push(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
        })),
        Step::Wait(2),
        Step::Do(Box::new(|world, editor| {
            editor.selection = find(world, WALKER).into_iter().collect();
        })),
        Step::Tab(Tab::Console),
        Step::Wait(30),
        Step::Shot("27_clip_dropped"),
        // The clip picker: the FBX horse trots.
        Step::Do(Box::new(|world, editor| {
            play(world, editor, FBX_HORSE, "Armature|Trot");
            editor.selection = find(world, FBX_HORSE).into_iter().collect();
        })),
        Step::Wait(17),
        Step::Shot("28_trot"),
        // Save, clear, reopen: the rigs come back from their files, still playing their clips.
        Step::Do(Box::new(|world, editor| {
            editor::save_scene(world, editor, SCENE);
            editor::new_scene(world, editor);
        })),
        Step::Wait(4),
        Step::Do(Box::new(|world, editor| {
            editor::load_scene(world, editor, SCENE);
            editor.selection = find(world, WALKER).into_iter().collect();
        })),
        Step::Wait(30),
        Step::Shot("29_reopened"),
    ];
    ShotScript::new(dir, steps)
}

/// Round 7: the animation timeline. A Rocketbox walker plays a walk clip in place; the clip
/// is scrubbed to four moments of one stride (a filmstrip); the horse behind keeps walking.
pub fn round7(dir: impl Into<PathBuf>) -> ShotScript {
    const WALKER: &str = "fetched/anim/characters/walker.glb";
    const CLIP: &str = "fetched/anim/clips/m_walk_neutral_01.glb";
    const HORSE: &str = "fetched/anim/animals/horse/Horse.gltf";
    fn find(world: &mut World, key: &str) -> Option<Entity> {
        let mut q = world.query::<(Entity, &crate::models::Model)>();
        q.iter(world).find(|(_, m)| m.path == key).map(|(e, _)| e)
    }
    fn scrub_to(secs: f32) -> Step {
        Step::Do(Box::new(move |world, _| {
            if let Some(walker) = find(world, WALKER) {
                crate::rigs::scrub(world, walker, secs);
            }
        }))
    }
    let mut steps = vec![
        // From the side, so the stride reads.
        Step::Camera(DVec3::new(3.4, 1.2, 0.3), DVec3::new(0.0, 0.9, 0.3)),
        Step::Select(""),
        Step::Tab(Tab::Console),
        Step::Do(Box::new(|world, editor| {
            crate::models::place(world, editor, WALKER, DVec3::ZERO);
            crate::models::place(world, editor, HORSE, DVec3::new(0.0, 0.0, -3.0));
            if let Some(walker) = find(world, WALKER) {
                crate::rigs::add_clip_file(world, editor, walker, CLIP);
            }
        })),
        Step::Wait(4),
        Step::Do(Box::new(|world, editor| {
            if let Some(walker) = find(world, WALKER) {
                crate::rigs::play_in_place(world, editor, walker, true);
                editor.selection = vec![walker];
            }
        })),
        Step::Wait(30),
        Step::Shot("30_in_place_playing"),
    ];
    for (secs, name) in [
        (0.0, "31_scrub_a"),
        (0.25, "31_scrub_b"),
        (0.5, "31_scrub_c"),
        (0.75, "31_scrub_d"),
    ] {
        steps.extend([scrub_to(secs), Step::Wait(6), Step::Shot(name)]);
    }
    ShotScript::new(dir, steps)
}

/// Round 8: the Content panel's model grid with thumbnails drawn on a worker thread.
pub fn round8(dir: impl Into<PathBuf>) -> ShotScript {
    let steps = vec![
        Step::Camera(DVec3::new(9.0, 3.2, 13.0), DVec3::new(2.0, 0.8, 2.0)),
        Step::Select(""),
        // A taller Content panel, as one would drag it while browsing assets.
        Step::Do(Box::new(|world, _| {
            world.insert_resource(crate::ui::EditorDock(crate::ui::layout(0.45)));
        })),
        Step::Tab(Tab::Content),
        // Every model in the asset folder is drawn once; later sessions read the cache.
        Step::Wait(60),
        Step::Shot("34_content_thumbnails"),
        Step::Do(Box::new(|world, _| {
            if let Some(mut thumbs) = world.get_resource_mut::<crate::thumbs::Thumbs>() {
                thumbs.tile = 120.0;
            }
        })),
        Step::Wait(4),
        Step::Shot("35_large_thumbnails"),
    ];
    ShotScript::new(dir, steps)
}

/// Round 9: selection outlines. A rig, a model behind a wall, a parent with children and a
/// multiple selection.
pub fn round9(dir: impl Into<PathBuf>) -> ShotScript {
    let steps = vec![
        Step::Camera(DVec3::new(5.5, 2.6, 7.5), DVec3::new(-0.5, 0.9, 0.0)),
        Step::Tab(Tab::Inspector),
        Step::Do(Box::new(|world, editor| {
            crate::models::place(
                world,
                editor,
                "fetched/anim/animals/horse/Horse.gltf",
                DVec3::new(-1.0, 0.0, 1.0),
            );
            crate::models::place(
                world,
                editor,
                "fetched/anim/characters/walker.glb",
                DVec3::new(-1.5, 0.0, -8.0),
            );
        })),
        Step::Select("Horse"),
        Step::Wait(30),
        Step::Shot("36_outline_rig"),
        // The walker stands behind the back wall: a faint line shows where.
        Step::Camera(DVec3::new(0.8, 1.7, -1.5), DVec3::new(-1.5, 1.0, -8.0)),
        Step::Select("walker"),
        Step::Wait(30),
        Step::Shot("37_outline_behind_wall"),
        // A parent outlines its children with it.
        Step::Camera(DVec3::new(3.5, 3.0, 4.5), DVec3::new(-2.5, 0.8, -3.5)),
        Step::Select("Crate stack"),
        Step::Wait(30),
        Step::Shot("38_outline_children"),
        // Several selected: the active one (picked last) is lighter.
        Step::Camera(DVec3::new(6.0, 4.0, 8.0), DVec3::new(-0.5, 0.6, -1.0)),
        Step::Do(Box::new(|world, editor| {
            editor.selection = ["Barrel", "Boulder", "Lamp post", "Horse"]
                .into_iter()
                .filter_map(|name| find(world, name))
                .collect();
        })),
        Step::Wait(30),
        Step::Shot("39_outline_multiple"),
        Step::Select("Horse"),
        Step::Camera(DVec3::new(4.0, 2.2, 5.5), DVec3::new(-1.0, 0.9, 0.5)),
        Step::Do(Box::new(editor::play)),
        Step::Wait(60),
        Step::Shot("40_outline_playing"),
        Step::Do(Box::new(editor::stop)),
    ];
    ShotScript::new(dir, steps)
}

/// Round 10: a camera's preview, pinned while the horse is selected, then piloted.
pub fn round10(dir: impl Into<PathBuf>) -> ShotScript {
    let shot_from =
        Transform::from_xyz(5.5, 1.6, 5.0).looking_at(DVec3::new(-1.0, 0.9, 0.5), Vec3::Y);
    let steps = vec![
        Step::Camera(DVec3::new(-7.0, 4.5, 9.0), DVec3::new(1.5, 0.8, 0.5)),
        Step::Tab(Tab::Inspector),
        Step::Do(Box::new(move |world, editor| {
            crate::models::place(
                world,
                editor,
                "fetched/anim/animals/horse/Horse.gltf",
                DVec3::new(-1.0, 0.0, 1.0),
            );
            let camera = editor::create(world, editor, editor::Create::Camera, DVec3::ZERO);
            if let Some(mut t) = world.get_mut::<Transform>(camera) {
                *t = shot_from;
            }
            editor.select(Some(camera));
        })),
        Step::Wait(40),
        Step::Shot("41_camera_preview"),
        // A longer lens shows at once.
        Step::Do(Box::new(|world, editor| {
            if let Some(e) = editor.primary()
                && let Some(mut c) = world.get_mut::<Camera>(e)
            {
                c.projection = Projection::Perspective {
                    fov_y_deg: 28.0,
                    near: 0.05,
                };
            }
        })),
        Step::Wait(20),
        Step::Shot("42_camera_preview_long_lens"),
        // Pinned, it stays while the horse is selected.
        Step::Do(Box::new(|world, editor| {
            let camera = editor.primary();
            world
                .resource_mut::<crate::camera_preview::CameraPreview>()
                .pinned = camera;
        })),
        Step::Select("Horse"),
        Step::Wait(20),
        Step::Shot("43_camera_preview_pinned"),
        // Pilot: the view looks through the camera, and flying moves it.
        Step::Select("Camera"),
        Step::Do(Box::new(|world, editor| {
            world
                .resource_mut::<crate::camera_preview::CameraPreview>()
                .pinned = None;
            if let Some(camera) = editor.primary() {
                crate::camera_preview::start_pilot(world, editor, camera);
            }
        })),
        Step::Wait(20),
        Step::Shot("44_pilot_through_camera"),
        Step::Camera(DVec3::new(2.5, 1.2, 3.5), DVec3::new(-1.0, 1.0, 0.5)),
        Step::Wait(20),
        Step::Shot("45_pilot_moved_in"),
        Step::Do(Box::new(|world, editor| {
            crate::camera_preview::stop_pilot(world, editor);
        })),
        Step::Camera(DVec3::new(-7.0, 4.5, 9.0), DVec3::new(1.5, 0.8, 0.5)),
        Step::Wait(30),
        Step::Shot("46_after_eject"),
    ];
    ShotScript::new(dir, steps)
}

/// Round 11: four texture sets from different tools (a folder dropped on the editor) become
/// materials, then go onto the courtyard. `ROUND11_SETS` is the folder.
pub fn round11(dir: impl Into<PathBuf>) -> ShotScript {
    let steps = vec![
        Step::Camera(DVec3::new(7.5, 3.2, 8.5), DVec3::new(-1.0, 0.8, -1.5)),
        Step::Select(""),
        Step::Do(Box::new(|world, editor| {
            let sphere = editor::shape_presets()
                .into_iter()
                .find(|(name, _)| *name == "Sphere")
                .map(|(name, shape)| editor::Create::Shape(name, shape));
            if let Some(what) = sphere {
                let e = editor::create(world, editor, what, DVec3::new(0.5, 0.0, 1.5));
                if let Some(mut t) = world.get_mut::<Transform>(e) {
                    t.translation.y = 0.8;
                    t.scale = Vec3::splat(1.6);
                }
            }
            let cube = editor::shape_presets()
                .into_iter()
                .find(|(name, _)| *name == "Cube")
                .map(|(name, shape)| editor::Create::Shape(name, shape));
            if let Some(what) = cube {
                let e = editor::create(world, editor, what, DVec3::new(-1.2, 0.0, 1.0));
                if let Some(mut t) = world.get_mut::<Transform>(e) {
                    t.translation.y = 0.5;
                }
            }
            editor.select(None);
        })),
        Step::Wait(30),
        Step::Shot("47_before_materials"),
        // The folder of texture sets, dropped on the editor.
        Step::Do(Box::new(|world, editor| {
            let source = PathBuf::from(std::env::var("ROUND11_SETS").unwrap_or_default());
            crate::import::import_paths(world, editor, &[source], None, None);
            editor.browser.search = "materials/".into();
            world.insert_resource(crate::ui::EditorDock(crate::ui::layout(0.5)));
        })),
        Step::Tab(Tab::Content),
        Step::Wait(80),
        Step::Shot("48_texture_sets_to_materials"),
        Step::Tab(Tab::Console),
        Step::Wait(4),
        Step::Shot("49_what_each_set_became"),
        // Onto the courtyard: one each, and one on all six crates from the selection.
        Step::Do(Box::new(|world, editor| {
            world.insert_resource(crate::ui::EditorDock(crate::ui::default_layout()));
            for (material, object) in [
                ("ground054", "Courtyard floor"),
                ("cliff_rock", "Back wall"),
                ("cliff_rock", "Side wall"),
                ("mud_wet_ukmkdc2", "Ramp"),
                ("rock_face", "Sphere"),
                ("rock_face", "Cube"),
                ("rock_face", "Barrel"),
            ] {
                let key = format!("imported/materials/{material}/{material}.material.gltf");
                if let Some(e) = find(world, object) {
                    crate::materials::apply(world, editor, &key, &[e]);
                }
            }
            editor.selection = find(world, "Crate stack").into_iter().collect();
            let crates = crate::materials::selected_meshes(world, editor);
            crate::materials::apply(
                world,
                editor,
                "imported/materials/rock_face/rock_face.material.gltf",
                &crates,
            );
        })),
        Step::Tab(Tab::History),
        Step::Wait(60),
        Step::Shot("50_materials_on_the_courtyard"),
        Step::Camera(DVec3::new(-1.2, 1.3, -1.6), DVec3::new(-2.5, 0.9, -3.5)),
        Step::Wait(20),
        Step::Shot("52_crates_close"),
        Step::Select("Sphere"),
        Step::Camera(DVec3::new(3.0, 1.6, 3.6), DVec3::new(0.3, 0.8, 1.2)),
        Step::Tab(Tab::Content),
        Step::Wait(30),
        Step::Shot("51_close_up"),
    ];
    ShotScript::new(dir, steps)
}

/// The well model round 12 imports.
const WELL: &str = "imported/stonewell/StoneWell.glb";

/// The well's import settings as saved.
fn well_settings(world: &World) -> crate::import::ImportSettings {
    let root = world
        .get_resource::<engine::assets::AssetServer>()
        .map(|s| s.root())
        .unwrap_or_default();
    crate::import::settings_for(&root, WELL).unwrap_or_default()
}

/// Round 12: a model's materials find their textures by name. `ROUND12_MODEL` is a folder
/// holding `StoneWell.fbx` (named materials, no textures, as Blender exports for Substance
/// Painter) and `Textures/StoneWell_<material>_<map>` texture sets.
pub fn round12(dir: impl Into<PathBuf>) -> ShotScript {
    let steps = vec![
        Step::Camera(DVec3::new(5.2, 2.9, 6.4), DVec3::new(1.6, 1.0, 1.8)),
        Step::Select(""),
        // The folder dropped on the editor; then the well the way the file alone draws it.
        Step::Do(Box::new(|world, editor| {
            let folder = PathBuf::from(std::env::var("ROUND12_MODEL").unwrap_or_default());
            crate::import::import_paths(
                world,
                editor,
                &[folder],
                Some(DVec3::new(1.6, 0.0, 1.8)),
                None,
            );
            let mut settings = well_settings(world);
            settings.materials.clear();
            settings.summary = settings
                .summary
                .split("; ")
                .next()
                .unwrap_or_default()
                .into();
            editor.browser.search = "stonewell".into();
            crate::import::save_remaps(world, editor, WELL, &settings);
        })),
        Step::Tab(Tab::Console),
        Step::Wait(60),
        Step::Shot("53_as_the_file_has_it"),
        // Search and remap: the Painter texture sets beside it, by material name.
        Step::Do(Box::new(|world, editor| {
            let settings = well_settings(world);
            crate::import::search_textures(world, editor, WELL, settings);
        })),
        Step::Wait(80),
        Step::Shot("54_textures_found_by_name"),
        // A remap by hand in the Details list: the cobbles get round 11's ground.
        Step::Do(Box::new(|world, editor| {
            let ground = "imported/materials/ground054/ground054.material.gltf";
            let mut settings = well_settings(world);
            settings.materials.insert("Cobbles".into(), ground.into());
            crate::import::save_remaps(world, editor, WELL, &settings);
        })),
        Step::Tab(Tab::History),
        Step::Wait(60),
        Step::Shot("55_remapped_by_hand"),
        Step::Camera(DVec3::new(3.2, 1.9, 3.9), DVec3::new(1.6, 1.0, 1.8)),
        Step::Tab(Tab::Content),
        Step::Wait(30),
        Step::Shot("56_close_up"),
    ];
    ShotScript::new(dir, steps)
}

/// Where round 13's wells stand: a box collider, convex parts.
const BOX_WELL: DVec3 = DVec3::new(2.4, 0.0, 2.2);
const PARTS_WELL: DVec3 = DVec3::new(-1.2, 0.0, 2.6);

/// Round 13: colliders that fit a model. Two copies of round 12's well (`ROUND12_MODEL`),
/// one made solid with a box, one with convex parts; balls dropped into each in play.
pub fn round13(dir: impl Into<PathBuf>) -> ShotScript {
    fn well(world: &mut World, editor: &mut Editor, kind: &str, at: DVec3, name: &'static str) {
        let mut settings = well_settings(world);
        settings.collider = kind.into();
        let root = world
            .get_resource::<engine::assets::AssetServer>()
            .map(|s| s.root())
            .unwrap_or_default();
        crate::import::save_settings(&root, WELL, &settings).ok();
        if let Some(e) = crate::models::place(world, editor, WELL, at) {
            world.entity_mut(e).insert(Name::new(name));
            crate::inspector::add_physics(world, editor, e, false);
        }
    }
    fn balls(world: &mut World, editor: &mut Editor, at: DVec3) {
        let sphere = editor::shape_presets()
            .into_iter()
            .find(|(name, _)| *name == "Sphere");
        for (i, (dx, dz)) in [(-0.3, -0.15), (0.25, 0.2), (0.0, 0.35), (-0.2, 0.3)]
            .into_iter()
            .enumerate()
        {
            let Some((name, shape)) = sphere else {
                return;
            };
            let p = at + DVec3::new(dx, 1.35 + 0.25 * i as f64, dz);
            let e = editor::create(world, editor, Create::Shape(name, shape), p);
            if let Some(mut t) = world.get_mut::<Transform>(e) {
                t.translation = p;
                t.scale = Vec3::splat(0.26);
            }
            world.entity_mut(e).insert((
                engine::world::physics::RigidBody::dynamic(),
                engine::world::physics::Collider::sphere(0.5),
            ));
        }
    }
    let steps = vec![
        Step::Camera(DVec3::new(0.7, 4.2, 8.6), DVec3::new(0.6, 0.9, 2.3)),
        Step::Select(""),
        Step::Do(Box::new(|world, editor| {
            let folder = PathBuf::from(std::env::var("ROUND12_MODEL").unwrap_or_default());
            crate::import::import_file(
                &world
                    .get_resource::<engine::assets::AssetServer>()
                    .map(|s| s.root())
                    .unwrap_or_default(),
                &folder.join("StoneWell.fbx"),
            )
            .ok();
            well(world, editor, "box", BOX_WELL, "Well (box)");
            well(
                world,
                editor,
                "convex parts",
                PARTS_WELL,
                "Well (convex parts)",
            );
            balls(world, editor, BOX_WELL);
            balls(world, editor, PARTS_WELL);
            editor.selection = ["Well (box)", "Well (convex parts)"]
                .into_iter()
                .filter_map(|n| find(world, n))
                .collect();
        })),
        Step::Tab(Tab::Console),
        Step::Wait(60),
        Step::Shot("57_box_and_convex_parts"),
        Step::Do(Box::new(editor::play)),
        Step::Wait(120),
        Step::Shot("58_balls_dropped_in"),
        Step::Do(Box::new(editor::stop)),
        Step::Select("Well (convex parts)"),
        Step::Camera(
            DVec3::new(-0.6, 2.6, 4.9),
            PARTS_WELL + DVec3::new(0.0, 0.7, 0.0),
        ),
        Step::Wait(40),
        Step::Shot("59_convex_parts_close"),
    ];
    ShotScript::new(dir, steps)
}

/// The entity called `name` under `root`.
fn find_under(world: &World, root: Entity, name: &str) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if world.get::<Name>(e).is_some_and(|n| n.as_str() == name) {
            return Some(e);
        }
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
    }
    None
}

/// Select the part called `part` of the placed well.
fn select_part(world: &mut World, editor: &mut Editor, part: &str) {
    editor.selection = find(world, "StoneWell")
        .and_then(|well| find_under(world, well, part))
        .into_iter()
        .collect();
}

/// Edit material file `key` through the history, as the Details panel does.
fn edit_material(
    world: &mut World,
    editor: &mut Editor,
    key: &str,
    change: impl FnOnce(&mut crate::material_edit::Params),
) {
    let root = world
        .get_resource::<engine::assets::AssetServer>()
        .map(|s| s.root())
        .unwrap_or_default();
    let Ok((before, _)) = crate::material_edit::read(&root, key) else {
        return;
    };
    let mut after = before.clone();
    change(&mut after);
    let (Ok(before), Ok(after)) = (serde_json::to_value(before), serde_json::to_value(after))
    else {
        return;
    };
    editor::apply(
        world,
        editor,
        crate::undo::Command::Material {
            key: key.into(),
            before,
            after,
        },
    );
    crate::material_edit::flush_all(world);
}

const STONE: &str = "imported/materials/stonewell_stone/stonewell_stone.material.gltf";

/// Round 14: materials edited in Details. Round 12's well (`ROUND12_MODEL`): its stone gets
/// tiled and toned down, and the water and bucket, which live inside the model file, are made
/// editable and turned into water and metal.
pub fn round14(dir: impl Into<PathBuf>) -> ShotScript {
    let steps = vec![
        Step::Camera(DVec3::new(4.0, 2.3, 4.7), DVec3::new(1.6, 0.8, 1.8)),
        Step::Select(""),
        // The well as round 12 left it: stone untiled, water and bucket the file's own.
        Step::Do(Box::new(|world, editor| {
            let root = world
                .get_resource::<engine::assets::AssetServer>()
                .map(|s| s.root())
                .unwrap_or_default();
            let folder = PathBuf::from(std::env::var("ROUND12_MODEL").unwrap_or_default());
            crate::import::import_file(&root, &folder.join("StoneWell.fbx")).ok();
            if let Ok((mut p, _)) = crate::material_edit::read(&root, STONE) {
                p.tiling = [1.0, 1.0];
                p.tint = [1.0; 4];
                p.normal_strength = 1.0;
                crate::material_edit::save(&root, STONE, &p).ok();
            }
            let mut settings = well_settings(world);
            settings
                .materials
                .retain(|name, _| ["Cobbles", "Stone", "Wood"].contains(&name.as_str()));
            settings.collider = "box".into();
            crate::import::save_settings(&root, WELL, &settings).ok();
            crate::models::place(world, editor, WELL, DVec3::new(1.6, 0.0, 1.8));
            select_part(world, editor, "Wall");
        })),
        Step::Tab(Tab::History),
        Step::Wait(60),
        Step::Shot("60_stone_before"),
        Step::Do(Box::new(|world, editor| {
            edit_material(world, editor, STONE, |p| {
                p.tiling = [4.0, 1.5];
                p.tint = [0.8, 0.78, 0.76, 1.0];
                p.normal_strength = 1.6;
            });
        })),
        Step::Wait(50),
        Step::Shot("61_stone_tiled"),
        // Water and bucket out of the model, then edited.
        Step::Do(Box::new(|world, editor| {
            let root = world
                .get_resource::<engine::assets::AssetServer>()
                .map(|s| s.root())
                .unwrap_or_default();
            let names = crate::remap::file_materials(&root.join(WELL));
            let index = |n: &str| names.iter().position(|m| m.name == n);
            if let Some(i) = index("Water")
                && let Some(key) = crate::material_edit::make_editable(world, editor, WELL, i)
            {
                edit_material(world, editor, &key, |p| {
                    p.tint = [0.02, 0.07, 0.08, 1.0];
                    p.roughness = 0.03;
                    p.metallic = 0.0;
                });
            }
            if let Some(i) = index("Bucket_Metal")
                && let Some(key) = crate::material_edit::make_editable(world, editor, WELL, i)
            {
                edit_material(world, editor, &key, |p| {
                    p.tint = [0.62, 0.6, 0.56, 1.0];
                    p.roughness = 0.32;
                    p.metallic = 1.0;
                });
            }
            select_part(world, editor, "Bucket");
        })),
        Step::Wait(80),
        Step::Shot("62_water_and_bucket"),
        Step::Camera(DVec3::new(2.6, 1.7, 3.4), DVec3::new(1.6, 0.7, 1.8)),
        Step::Do(Box::new(|world, editor| {
            select_part(world, editor, "Water")
        })),
        Step::Wait(40),
        Step::Shot("63_close_up"),
    ];
    ShotScript::new(dir, steps)
}

/// Imported keys of round 15's models, by file stem.
fn nature(world: &World, stem: &str) -> String {
    let root = world
        .get_resource::<engine::assets::AssetServer>()
        .map(|s| s.root())
        .unwrap_or_default();
    let folder = PathBuf::from(std::env::var("ROUND15_MODELS").unwrap_or_default());
    crate::import::import_file(&root, &folder.join(format!("{stem}.fbx"))).unwrap_or_default()
}

/// One brush stroke through `points` (x, z), as the scatter brush paints while dragging.
fn stroke(world: &mut World, editor: &mut Editor, points: &[(f64, f64)], erase: bool) {
    for &(x, z) in points {
        crate::scatter::dab(world, editor, DVec3::new(x, 0.0, z), erase);
    }
    crate::scatter::end_stroke(world, editor);
}

/// Round 15: the scatter brush. Rocks, stumps and small pines made in Blender
/// (`ROUND15_MODELS`: Rock_A/B/C, Stump, Pine .fbx and a Textures folder) painted onto the
/// ground beside the courtyard, then a patch erased.
pub fn round15(dir: impl Into<PathBuf>) -> ShotScript {
    let steps = vec![
        Step::Camera(DVec3::new(15.0, 7.5, 23.0), DVec3::new(7.0, 0.0, 9.0)),
        Step::Select(""),
        Step::Do(Box::new(|world, editor| {
            let rocks: Vec<String> = ["Rock_A", "Rock_B", "Rock_C"]
                .iter()
                .map(|s| nature(world, s))
                .collect();
            nature(world, "Stump");
            nature(world, "Pine");
            editor.models = None;
            editor.browser.search = "imported".into();
            editor.tool = Tool::Scatter;
            editor.scatter.palette = rocks;
        })),
        Step::Tab(Tab::History),
        Step::Wait(40),
        Step::Shot("64_scatter_brush"),
        Step::Do(Box::new(|world, editor| {
            let s = &mut editor.scatter;
            s.radius = 4.5;
            s.density = 14.0;
            s.scale = [0.5, 1.5];
            s.align = true;
            s.sink = 0.08;
            let path: Vec<(f64, f64)> = (0..8)
                .map(|i| (1.5 + 2.0 * i as f64, 7.5 + (i as f64 * 0.9).sin() * 1.5))
                .collect();
            stroke(world, editor, &path, false);
            let pine = nature(world, "Pine");
            let stump = nature(world, "Stump");
            let s = &mut editor.scatter;
            s.palette = vec![pine.clone(), pine, stump];
            s.radius = 3.5;
            s.density = 9.0;
            s.scale = [0.6, 1.2];
            s.align = false;
            s.sink = 0.02;
            let path: Vec<(f64, f64)> = (0..6).map(|i| (3.0 + 2.6 * i as f64, 2.5)).collect();
            stroke(world, editor, &path, false);
        })),
        Step::Wait(90),
        Step::Shot("65_painted"),
        Step::Do(Box::new(|world, editor| {
            // Erase only the rocks: the picked models are the ones erased.
            let mut groups = world.query::<&crate::scatter::ScatterGroup>();
            editor.scatter.palette = groups
                .iter(world)
                .map(|g| g.model.clone())
                .filter(|m| m.contains("Rock_"))
                .collect();
            editor.scatter.radius = 3.0;
            stroke(world, editor, &[(9.0, 8.5), (10.0, 9.5)], true);
        })),
        Step::Wait(40),
        Step::Shot("66_patch_erased"),
        Step::Camera(DVec3::new(5.0, 1.5, 12.0), DVec3::new(10.0, 0.6, 5.0)),
        Step::Wait(40),
        Step::Shot("67_close_up"),
    ];
    ShotScript::new(dir, steps)
}

/// The corner of `name`'s mesh bounds furthest along `dir`.
fn corner(world: &mut World, name: &str, dir: DVec3) -> DVec3 {
    let e = find(world, name).expect("entity");
    crate::snap::box_corners(world, e)
        .into_iter()
        .max_by(|a, b| a.dot(dir).total_cmp(&b.dot(dir)))
        .unwrap_or_default()
}

/// Where `at` is on screen.
fn screen(editor: &Editor, at: DVec3) -> egui::Pos2 {
    editor
        .view
        .and_then(|v| v.project(at))
        .unwrap_or(egui::pos2(800.0, 450.0))
}

/// Queue input events for the next frame, as a user's hands would make them.
fn inject(world: &mut World, events: Vec<egui::Event>) {
    world
        .resource_mut::<crate::egui_bridge::EguiInput>()
        .injected
        .extend(events);
}

fn key_v(pressed: bool) -> egui::Event {
    egui::Event::Key {
        key: egui::Key::V,
        physical_key: Some(egui::Key::V),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn button(at: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    }
}

/// Round 16: snapping. A crate's corner dragged onto another crate's corner with V held, then
/// the barrel dragged up the ramp with Ctrl+Shift, tilted to its slope.
pub fn round16(dir: impl Into<PathBuf>) -> ShotScript {
    // The front-left-bottom corner of the loose crate onto the front-right-bottom of Crate 3.
    const GRAB: DVec3 = DVec3::new(-1.0, -1.0, 1.0);
    const ONTO: DVec3 = DVec3::new(1.0, -1.0, 1.0);
    let both = egui::Modifiers {
        ctrl: true,
        shift: true,
        command: true,
        ..egui::Modifiers::NONE
    };
    let steps = vec![
        Step::Camera(DVec3::new(1.2, 2.4, 3.2), DVec3::new(-0.9, 0.5, -3.0)),
        Step::Select("Crate 6"),
        Step::Tool(Tool::Move),
        Step::Tab(Tab::History),
        Step::Do(Box::new(|world, _| {
            // Crate 6 off the stack, loose on the ground to the right.
            let e = find(world, "Crate 6").expect("crate");
            if let Some(mut t) = world.get_mut::<Transform>(e) {
                t.translation = DVec3::new(2.9, 0.45, 1.6);
                t.rotation = Quat::from_rotation_y(0.35);
            }
        })),
        Step::Wait(30),
        Step::Do(Box::new(|world, editor| {
            let at = screen(editor, corner(world, "Crate 6", GRAB));
            inject(world, vec![egui::Event::PointerMoved(at), key_v(true)]);
        })),
        Step::Wait(4),
        Step::Shot("68_hold_v_grabs_a_corner"),
        Step::Do(Box::new(|world, editor| {
            let at = screen(editor, corner(world, "Crate 6", GRAB));
            inject(world, vec![button(at, true, egui::Modifiers::NONE)]);
        })),
        Step::Wait(2),
        Step::Do(Box::new(|world, editor| {
            // Near the other crate's corner, not on it: the snap pulls it in.
            let at = screen(editor, corner(world, "Crate 3", ONTO)) + egui::vec2(9.0, -6.0);
            inject(world, vec![egui::Event::PointerMoved(at)]);
        })),
        Step::Wait(4),
        Step::Shot("69_dragged_onto_a_corner"),
        Step::Do(Box::new(|world, editor| {
            let at = screen(editor, corner(world, "Crate 3", ONTO)) + egui::vec2(9.0, -6.0);
            inject(
                world,
                vec![button(at, false, egui::Modifiers::NONE), key_v(false)],
            );
        })),
        Step::Wait(4),
        Step::Do(Box::new(|world, _| {
            let a = corner(world, "Crate 6", GRAB);
            let b = corner(world, "Crate 3", ONTO);
            engine::core::log::info!("corner gap after the snap: {:.4} m", (a - b).length());
        })),
        Step::Camera(DVec3::new(-0.4, 1.3, -1.2), DVec3::new(-1.3, 0.4, -2.9)),
        Step::Wait(30),
        Step::Shot("70_corners_meet"),
        // Surface snap: the barrel up onto the ramp.
        Step::Camera(DVec3::new(9.5, 3.4, 3.0), DVec3::new(3.2, 0.6, -2.6)),
        Step::Select("Barrel"),
        Step::Do(Box::new(|_, editor| {
            editor.surface_align = true;
        })),
        Step::Wait(30),
        Step::Do(Box::new(move |world, editor| {
            let e = find(world, "Barrel").expect("barrel");
            let at = world
                .get::<GlobalTransform>(e)
                .map(|g| screen(editor, g.translation()))
                .unwrap_or_default();
            inject(
                world,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::ModifiersChanged(both),
                ],
            );
        })),
        Step::Wait(2),
        Step::Do(Box::new(move |world, editor| {
            let e = find(world, "Barrel").expect("barrel");
            let at = world
                .get::<GlobalTransform>(e)
                .map(|g| screen(editor, g.translation()))
                .unwrap_or_default();
            inject(world, vec![button(at, true, both)]);
        })),
        Step::Wait(2),
        Step::Do(Box::new(|world, editor| {
            let ramp = find(world, "Ramp").expect("ramp");
            let at = world
                .get::<GlobalTransform>(ramp)
                .map(|g| screen(editor, g.translation() + DVec3::new(0.6, 0.0, 0.0)))
                .unwrap_or_default();
            inject(world, vec![egui::Event::PointerMoved(at)]);
        })),
        Step::Wait(4),
        Step::Shot("71_ctrl_shift_slides_onto_the_ramp"),
        Step::Do(Box::new(move |world, editor| {
            let ramp = find(world, "Ramp").expect("ramp");
            let at = world
                .get::<GlobalTransform>(ramp)
                .map(|g| screen(editor, g.translation() + DVec3::new(0.6, 0.0, 0.0)))
                .unwrap_or_default();
            inject(
                world,
                vec![
                    button(at, false, both),
                    egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                ],
            );
        })),
        Step::Select(""),
        Step::Wait(30),
        Step::Shot("72_barrel_on_the_slope"),
    ];
    ShotScript::new(dir, steps)
}

/// Click the axis widget: the end of `axis` (or the middle), as a mouse would.
fn click_axis_widget(axis: Option<Vec3>) -> Vec<Step> {
    let at = move |editor: &Editor| {
        let view = editor.view.expect("a scene view");
        let center = crate::viewport::axis_widget_center(view.screen);
        match axis {
            Some(a) => {
                let v = view.rotation.inverse() * a;
                center + egui::vec2(v.x, -v.y) * 30.0
            }
            None => center,
        }
    };
    vec![
        Step::Do(Box::new(move |world, editor| {
            let p = at(editor);
            inject(world, vec![egui::Event::PointerMoved(p)]);
        })),
        Step::Wait(2),
        Step::Do(Box::new(move |world, editor| {
            let p = at(editor);
            inject(world, vec![button(p, true, egui::Modifiers::NONE)]);
        })),
        Step::Wait(1),
        Step::Do(Box::new(move |world, editor| {
            let p = at(editor);
            inject(world, vec![button(p, false, egui::Modifiers::NONE)]);
        })),
        Step::Wait(30),
    ]
}

/// Round 17: orthographic views from the axis widget. The courtyard in perspective, then a
/// click on the widget's Y for the top view, Z for the front, X for the right side, and the
/// middle back to perspective.
pub fn round17(dir: impl Into<PathBuf>) -> ShotScript {
    let mut steps = vec![
        Step::Camera(DVec3::new(9.0, 6.5, 9.0), DVec3::new(0.0, 0.6, -2.0)),
        Step::Select("Barrel"),
        Step::Tool(Tool::Move),
        Step::Wait(30),
        Step::Shot("73_perspective"),
    ];
    steps.extend(click_axis_widget(Some(Vec3::Y)));
    steps.push(Step::Shot("74_top"));
    steps.extend(click_axis_widget(Some(Vec3::Z)));
    steps.push(Step::Shot("75_front"));
    steps.extend(click_axis_widget(Some(Vec3::X)));
    steps.push(Step::Shot("76_right"));
    steps.extend(click_axis_widget(None));
    steps.push(Step::Shot("77_right_in_perspective"));
    ShotScript::new(dir, steps)
}

/// Finds a screen position when a scripted step runs.
type ScreenPointFn = dyn Fn(&mut World, &Editor) -> egui::Pos2 + Send + Sync;
type ScreenPoint = Box<ScreenPointFn>;

/// A primary-button drag through `points` (screen positions found when each step runs), with
/// `modifiers` held, as a hand on the mouse would make it.
fn drag(points: Vec<ScreenPoint>, modifiers: egui::Modifiers) -> Vec<Step> {
    let points: Vec<std::sync::Arc<ScreenPointFn>> =
        points.into_iter().map(std::sync::Arc::from).collect();
    let mut steps = Vec::new();
    let first = points[0].clone();
    steps.push(Step::Do(Box::new(move |world, editor| {
        let p = first(world, editor);
        inject(
            world,
            vec![
                egui::Event::PointerMoved(p),
                egui::Event::ModifiersChanged(modifiers),
            ],
        );
    })));
    steps.push(Step::Wait(3));
    let first = points[0].clone();
    steps.push(Step::Do(Box::new(move |world, editor| {
        let p = first(world, editor);
        inject(world, vec![button(p, true, modifiers)]);
    })));
    steps.push(Step::Wait(2));
    for point in points.iter().skip(1) {
        let point = point.clone();
        steps.push(Step::Do(Box::new(move |world, editor| {
            let p = point(world, editor);
            inject(world, vec![egui::Event::PointerMoved(p)]);
        })));
        steps.push(Step::Wait(2));
    }
    let last = points[points.len() - 1].clone();
    steps.push(Step::Do(Box::new(move |world, editor| {
        let p = last(world, editor);
        inject(
            world,
            vec![
                button(p, false, modifiers),
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            ],
        );
    })));
    steps.push(Step::Wait(3));
    steps
}

/// Round 18: Alt+drag on the move gizmo leaves copies behind (a row of lamp posts), then the
/// Measure tool: the posts' spacing, and the crate stack's height in the front view.
pub fn round18(dir: impl Into<PathBuf>) -> ShotScript {
    // The gizmo's X arrow, about 55 points out from its middle, on a post standing `start`
    // meters east of the first one, pulled `step` meters further.
    fn x_arrow(start: f64, step: f64) -> ScreenPoint {
        Box::new(move |world, editor| {
            let first = find(world, "Lamp post").expect("lamp post");
            let p = world
                .get::<GlobalTransform>(first)
                .map(|g| g.translation())
                .unwrap_or_default()
                + DVec3::X * start;
            let a = screen(editor, p);
            let b = screen(editor, p + DVec3::X);
            let arrow = a + (b - a).normalized() * 55.0;
            arrow + (screen(editor, p + DVec3::X * step) - a)
        })
    }
    let alt = egui::Modifiers {
        alt: true,
        ..egui::Modifiers::NONE
    };
    let mut steps = vec![
        Step::Camera(DVec3::new(9.5, 4.2, 9.0), DVec3::new(1.5, 1.0, 0.5)),
        Step::Select("Lamp post"),
        Step::Tool(Tool::Move),
        Step::Tab(Tab::History),
        Step::Wait(30),
        Step::Shot("78_one_lamp_post"),
    ];
    // Each drag leaves a copy and moves the selection (the newest copy) on by 3.5 m.
    for i in 0..2 {
        let start = 3.5 * i as f64;
        steps.extend(drag(
            vec![
                x_arrow(start, 0.0),
                x_arrow(start, 1.5),
                x_arrow(start, 3.5),
            ],
            alt,
        ));
    }
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("79_alt_drag_left_copies"));
    steps.push(Step::Tool(Tool::Measure));
    steps.push(Step::Select(""));
    steps.extend(drag(
        vec![
            Box::new(|world, editor| {
                // The ground at the foot of each post.
                let posts = lamp_posts(world);
                screen(editor, posts[0] * DVec3::new(1.0, 0.0, 1.0))
            }),
            Box::new(|world, editor| {
                let posts = lamp_posts(world);
                screen(editor, posts[posts.len() - 1] * DVec3::new(1.0, 0.0, 1.0))
            }),
        ],
        egui::Modifiers::NONE,
    ));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("80_measure_post_spacing"));
    // The crate stack's height, corner to corner, in the front view.
    steps.push(Step::Camera(
        DVec3::new(-2.5, 1.4, 4.0),
        DVec3::new(-2.5, 1.4, -3.5),
    ));
    steps.push(Step::Do(Box::new(|_, editor| {
        editor.view_request = Some(crate::camera::ViewRequest::Along(Vec3::NEG_Z));
    })));
    steps.push(Step::Wait(20));
    steps.extend(drag(
        vec![
            Box::new(|world, editor| {
                screen(
                    editor,
                    corner(world, "Crate 1", DVec3::new(-1.0, -1.0, 1.0)),
                )
            }),
            Box::new(|world, editor| {
                screen(editor, corner(world, "Crate 6", DVec3::new(-1.0, 1.0, 1.0)))
            }),
        ],
        egui::Modifiers::NONE,
    ));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("81_measure_stack_height_front"));
    ShotScript::new(dir, steps)
}

/// Where the lamp posts stand, west to east.
fn lamp_posts(world: &mut World) -> Vec<DVec3> {
    let mut q = world.query::<(&Name, &GlobalTransform)>();
    let mut posts: Vec<DVec3> = q
        .iter(world)
        .filter(|(n, _)| n.as_str().starts_with("Lamp post"))
        .map(|(_, g)| g.translation())
        .collect();
    posts.sort_by(|a, b| a.x.total_cmp(&b.x));
    posts
}

/// Click a toolbar button whose rect the toolbar recorded (`pick` finds it in the editor).
fn click_toolbar(pick: fn(&Editor) -> Option<egui::Rect>) -> Vec<Step> {
    let at = move |editor: &Editor| pick(editor).map_or(egui::pos2(0.0, 0.0), |r| r.center());
    vec![
        Step::Do(Box::new(move |world, editor| {
            let p = at(editor);
            inject(world, vec![egui::Event::PointerMoved(p)]);
        })),
        Step::Wait(2),
        Step::Do(Box::new(move |world, editor| {
            let p = at(editor);
            inject(world, vec![button(p, true, egui::Modifiers::NONE)]);
        })),
        Step::Wait(1),
        Step::Do(Box::new(move |world, editor| {
            let p = at(editor);
            inject(world, vec![button(p, false, egui::Modifiers::NONE)]);
        })),
        Step::Wait(4),
    ]
}

/// Round 19: the View and Show menus. The lit courtyard, the View menu open, the normals and
/// ambient occlusion buffers, the Show menu with physics colliders and wind on, what they draw,
/// and Game view with every editor helper hidden.
pub fn round19(dir: impl Into<PathBuf>) -> ShotScript {
    use engine::render::DebugView;
    let mut steps = vec![
        Step::Camera(DVec3::new(9.0, 6.5, 9.0), DVec3::new(0.0, 0.6, -2.0)),
        Step::Select("Barrel"),
        Step::Tool(Tool::Move),
        Step::Wait(30),
        Step::Shot("82_lit"),
    ];
    steps.extend(click_toolbar(|e| e.view_button));
    steps.push(Step::Shot("83_view_menu"));
    // Close it with a second click, then pick views as its items would.
    steps.extend(click_toolbar(|e| e.view_button));
    steps.push(Step::Do(Box::new(|world, _| {
        crate::show::set_view(world, DebugView::Normals);
    })));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("84_normals"));
    steps.push(Step::Do(Box::new(|world, _| {
        crate::show::set_view(world, DebugView::AmbientOcclusion);
    })));
    steps.push(Step::Wait(30));
    steps.push(Step::Shot("85_ambient_occlusion"));
    steps.push(Step::Do(Box::new(|world, _| {
        crate::show::set_view(world, DebugView::None);
        crate::show::set_flag(world, "PhysicsDebug", &["colliders"], true);
        crate::show::set_flag(world, "DebugViews", &["wind", "enabled"], true);
    })));
    steps.extend(click_toolbar(|e| e.show_button));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("86_show_menu"));
    steps.extend(click_toolbar(|e| e.show_button));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("87_colliders_and_wind"));
    steps.push(Step::Do(Box::new(|world, editor| {
        crate::show::set_flag(world, "PhysicsDebug", &["colliders"], false);
        crate::show::set_flag(world, "DebugViews", &["wind", "enabled"], false);
        editor.game_view = true;
    })));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("88_game_view"));
    ShotScript::new(dir, steps)
}

/// Round 20: prefabs. Lamp posts as copies of one prefab: one copy's changes marked and
/// listed, one applied to all; a street of lamp posts nesting the prefab; the prefab edited in
/// place reaching every copy, nested ones too; a variant that follows its base.
pub fn round20(dir: impl Into<PathBuf>) -> ShotScript {
    use crate::prefab::{self, Item};
    const POST: &str = "prefabs/lamp_post.prefab.json";
    // Loose lamp post copies (not inside the street), west to east.
    fn loose(world: &mut World) -> Vec<Entity> {
        let mut q = world.query_filtered::<(Entity, &prefab::PrefabInstance, &GlobalTransform), Without<ChildOf>>();
        let mut found: Vec<(f64, Entity)> = q
            .iter(world)
            .filter(|(_, l, _)| l.path.starts_with("prefabs/lamp_post"))
            .map(|(e, _, g)| (g.translation().x, e))
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.into_iter().map(|(_, e)| e).collect()
    }
    fn lamp(world: &World, root: Entity) -> Entity {
        prefab::part_entities(world, root)
            .get("lamp")
            .copied()
            .unwrap_or(root)
    }
    // A lamp's light and its glowing bulb.
    fn light_color(world: &mut World, e: Entity, color: [f32; 3]) {
        if let Some(mut l) = world.get_mut::<PointLight>(e) {
            l.color = color;
        }
        let [r, g, b] = color;
        let patch = world
            .resource::<TypeRegistry>()
            .component("MeshRenderer")
            .map(|c| c.patch);
        if let Some(patch) = patch {
            let _ = patch(
                world,
                e,
                serde_json::json!({"material": {"inline": {
                    "base_color": [r, g, b, 1.0],
                    "emissive": [r, g, b],
                    "emissive_strength": 8.0,
                }}}),
            );
        }
    }
    let mut steps = vec![
        Step::Do(Box::new(|world, editor| {
            // This script's own files from an earlier run.
            // Dusk, so the lamps' colors show; no icons over the bulbs.
            editor.icons = false;
            let sky = world
                .resource::<TypeRegistry>()
                .resource("Sky")
                .map(|r| r.patch);
            if let Some(patch) = sky {
                let _ = patch(
                    world,
                    serde_json::json!({"hour": 19.3, "hours_per_second": 0.0}),
                );
            }
            for f in prefab::prefab_files() {
                let name = f.rsplit('/').next().unwrap_or("");
                if name.starts_with("lamp_post") || name.starts_with("street_lamps") {
                    let _ = std::fs::remove_file(&f);
                }
            }
            let Some(post) = find(world, "Lamp post") else {
                return;
            };
            prefab::create(world, editor, post);
            for x in [-0.5, -3.5] {
                prefab::place(world, editor, POST, Some(DVec3::new(x, 0.0, 1.5)), None);
            }
            // The middle copy: a red, bigger lamp of its own.
            let middle = loose(world)[1];
            let l = lamp(world, middle);
            light_color(world, l, [1.0, 0.3, 0.15]);
            if let Some(mut t) = world.get_mut::<Transform>(l) {
                t.scale = Vec3::splat(1.6);
            }
            editor.select(Some(l));
        })),
        Step::Camera(DVec3::new(2.0, 5.0, 12.0), DVec3::new(-0.8, 1.7, 3.0)),
        Step::Tab(Tab::History),
        Step::Wait(40),
        Step::Shot("89_overrides_marked"),
        Step::Do(Box::new(|world, editor| {
            let middle = loose(world)[1];
            editor.select(Some(middle));
        })),
        Step::Wait(4),
    ];
    steps.extend(click_toolbar(|e| e.overrides_button));
    steps.push(Step::Wait(6));
    steps.push(Step::Shot("90_overrides_list"));
    steps.extend(click_toolbar(|e| e.overrides_button));
    // Apply the red light and bulb (as the list's ✔ would): every copy turns red, the size
    // stays the middle one's own.
    steps.push(Step::Do(Box::new(|world, editor| {
        let middle = loose(world)[1];
        for component in ["PointLight", "MeshRenderer"] {
            let item = Item::Field {
                part: "lamp".into(),
                component: component.into(),
                field: String::new(),
            };
            if let Some(target) = prefab::targets(world, middle, "lamp", component).first() {
                prefab::apply_item(world, editor, middle, &item, target);
            }
        }
    })));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("91_color_applied_to_all"));
    // A street: three lamp posts inside one entity, saved as a prefab that nests them, and a
    // second street placed behind.
    steps.push(Step::Do(Box::new(|world, editor| {
        let street = world
            .spawn((
                crate::scene_ops::InScene,
                crate::scene_ops::SceneOrder::next(),
                Name::new("Street lamps"),
                Transform::from_xyz(-1.0, 0.0, 4.0),
            ))
            .id();
        for x in [-3.0, 0.0, 3.0] {
            if let Some(p) = prefab::place(world, editor, POST, None, Some(street))
                && let Some(mut t) = world.get_mut::<Transform>(p)
            {
                t.translation.x = x;
            }
        }
        prefab::create(world, editor, street);
        prefab::place(
            world,
            editor,
            "prefabs/street_lamps.prefab.json",
            Some(DVec3::new(-1.0, 0.0, 6.5)),
            None,
        );
    })));
    // Edit the lamp post prefab in place: a blue lamp and a sign.
    steps.push(Step::Do(Box::new(|world, editor| {
        let east = *loose(world).last().expect("posts");
        prefab::enter(world, editor, east);
        let l = lamp(world, east);
        light_color(world, l, [0.4, 0.7, 1.0]);
        let sign = editor::create(
            world,
            editor,
            editor::Create::Shape(
                "Cube",
                Shape::Cuboid {
                    size: [0.6, 0.36, 0.06],
                },
            ),
            DVec3::ZERO,
        );
        editor::reparent(world, editor, sign, Some(east));
        if let Some(mut t) = world.get_mut::<Transform>(sign) {
            *t = Transform::from_xyz(0.33, 0.2, 0.0);
        }
        world.entity_mut(sign).insert(Name::new("Sign"));
        editor.select(Some(sign));
    })));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("92_editing_the_prefab"));
    steps.push(Step::Do(Box::new(|world, editor| {
        prefab::exit(world, editor);
        editor.select(None);
    })));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("93_every_copy_updated"));
    // A variant: the west copy goes green without its sign and is saved as a variant; then the
    // base gets a bigger lamp, which the variant takes while staying green.
    steps.push(Step::Do(Box::new(|world, editor| {
        let west = loose(world)[0];
        let l = lamp(world, west);
        light_color(world, l, [0.3, 1.0, 0.35]);
        if let Some(sign) = prefab::part_entities(world, west).get("sign").copied() {
            world.entity_mut(sign).despawn();
        }
        prefab::save_variant(world, editor, west);
        let east = *loose(world).last().expect("posts");
        prefab::enter(world, editor, east);
        let l = lamp(world, east);
        if let Some(mut t) = world.get_mut::<Transform>(l) {
            t.scale = Vec3::splat(1.5);
        }
        prefab::exit(world, editor);
        editor.select(Some(west));
    })));
    steps.push(Step::Wait(20));
    steps.push(Step::Shot("94_variant_follows_base"));
    ShotScript::new(dir, steps)
}

/// Round 21: the Content Browser. Tiles with pictures for every kind of asset beside the
/// folder tree and favorites, a search across everything, a prefab's users, renaming it while
/// its copies and the prefab nesting it follow, the right-click menu, and a delete that first
/// says what still uses the asset.
pub fn round21(dir: impl Into<PathBuf>) -> ShotScript {
    use crate::browser::{self, Act};
    use crate::content::{Asset, Root};
    use crate::prefab;
    const ROCK: &str = "imported/materials/rock_face/rock_face.material.gltf";
    fn post() -> Asset {
        Asset::new(Root::Prefabs, "lamp_post.prefab.json")
    }
    fn renamed() -> Asset {
        Asset::new(Root::Prefabs, "street_light.prefab.json")
    }
    fn mouse(world: &mut World, at: egui::Pos2, button: egui::PointerButton, pressed: bool) {
        inject(
            world,
            vec![egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }
    fn tile(editor: &Editor, a: &Asset) -> egui::Pos2 {
        editor
            .browser
            .tile_rects
            .get(a)
            .map_or(egui::pos2(0.0, 0.0), |r| r.center())
    }
    let steps = vec![
        Step::Do(Box::new(|world, editor| {
            // This script's own files from an earlier run.
            for f in prefab::prefab_files() {
                let name = f.rsplit('/').next().unwrap_or("");
                if ["lamp_post", "street_lamps", "street_light"]
                    .iter()
                    .any(|p| name.starts_with(p))
                {
                    let _ = std::fs::remove_file(&f);
                }
            }
            // A lamp post prefab, a loose copy, and a street of three that nests it.
            if let Some(lamp) = find(world, "Lamp post") {
                prefab::create(world, editor, lamp);
            }
            let path = post().key();
            prefab::place(world, editor, &path, Some(DVec3::new(-1.0, 0.0, 1.5)), None);
            let street = world
                .spawn((
                    crate::scene_ops::InScene,
                    crate::scene_ops::SceneOrder::next(),
                    Name::new("Street lamps"),
                    Transform::from_xyz(-1.0, 0.0, 5.0),
                ))
                .id();
            for x in [-3.0, 0.0, 3.0] {
                if let Some(p) = prefab::place(world, editor, &path, None, Some(street))
                    && let Some(mut t) = world.get_mut::<Transform>(p)
                {
                    t.translation.x = x;
                }
            }
            prefab::create(world, editor, street);
            // The rock material on three objects.
            for object in ["Sphere", "Barrel", "Boulder"] {
                if let Some(e) = find(world, object) {
                    crate::materials::apply(world, editor, ROCK, &[e]);
                }
            }
            editor.select(None);
            let favorites = editor.browser.favorites();
            *favorites = vec![
                Asset::new(Root::Assets, "imported/materials"),
                Asset::new(Root::Assets, "imported/rock_b/Rock_B.glb"),
                post(),
            ];
            crate::content::save_favorites(favorites);
            world.insert_resource(crate::ui::EditorDock(crate::ui::layout(0.42)));
            let rock = Asset::new(Root::Assets, ROCK);
            editor.browser.go(rock.parent());
            editor.browser.selected = Some(rock);
        })),
        Step::Camera(DVec3::new(6.0, 4.5, 11.0), DVec3::new(0.0, 0.8, 1.0)),
        Step::Tab(Tab::Content),
        Step::Wait(80),
        Step::Shot("95_tiles_tree_and_details"),
        Step::Do(Box::new(|_, editor| {
            editor.browser.search = "rock".into();
            editor.browser.types =
                vec![crate::content::Kind::Material, crate::content::Kind::Model];
            editor.browser.selected = Some(Asset::new(Root::Assets, "imported/rock_b/Rock_B.glb"));
        })),
        Step::Wait(80),
        Step::Shot("96_search_everything"),
        Step::Do(Box::new(|_, editor| {
            editor.browser.types.clear();
            editor.browser.go(Asset::new(Root::Prefabs, ""));
            editor.browser.selected = Some(post());
        })),
        Step::Wait(60),
        Step::Shot("97_prefab_and_its_users"),
        Step::Do(Box::new(|_, editor| {
            editor.browser.renaming = Some((post(), "street_light.prefab.json".into()));
        })),
        Step::Wait(6),
        Step::Shot("98_renaming"),
        Step::Do(Box::new(|world, editor| {
            editor.browser.renaming = None;
            browser::act(world, editor, Act::Rename(post(), "street_light".into()));
            editor.browser.selected = Some(renamed());
        })),
        Step::Wait(40),
        Step::Shot("99_renamed_and_users_follow"),
        Step::Do(Box::new(|_, editor| {
            let rock = Asset::new(Root::Assets, ROCK);
            editor.browser.go(rock.parent());
            editor.browser.selected = Some(rock);
        })),
        Step::Wait(20),
        Step::Do(Box::new(|world, editor| {
            let at = tile(editor, &Asset::new(Root::Assets, ROCK));
            inject(world, vec![egui::Event::PointerMoved(at)]);
        })),
        Step::Wait(2),
        Step::Do(Box::new(|world, editor| {
            let at = tile(editor, &Asset::new(Root::Assets, ROCK));
            mouse(world, at, egui::PointerButton::Secondary, true);
        })),
        Step::Wait(1),
        Step::Do(Box::new(|world, editor| {
            let at = tile(editor, &Asset::new(Root::Assets, ROCK));
            mouse(world, at, egui::PointerButton::Secondary, false);
        })),
        Step::Wait(6),
        Step::Shot("100_right_click_menu"),
        Step::Do(Box::new(|world, editor| {
            inject(
                world,
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: Some(egui::Key::Escape),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            browser::act(
                world,
                editor,
                Act::AskDelete(Asset::new(Root::Assets, ROCK)),
            );
        })),
        Step::Wait(10),
        Step::Shot("101_delete_says_what_uses_it"),
        Step::Do(Box::new(|_, editor| {
            editor.browser.deleting = None;
        })),
        Step::Wait(2),
    ];
    ShotScript::new(dir, steps)
}

/// Play in editor with a window: rigs and the physics courtyard play, stop puts the scene
/// back. For checking the windowed renderer (run under a real or virtual display).
pub fn play_in_editor(dir: impl Into<PathBuf>) -> ShotScript {
    let steps = vec![
        Step::Camera(DVec3::new(6.5, 3.0, 9.5), DVec3::new(0.5, 0.8, 0.0)),
        Step::Select(""),
        Step::Tab(Tab::Console),
        Step::Do(Box::new(|world, editor| {
            crate::models::place(
                world,
                editor,
                "fetched/anim/animals/horse/Horse.gltf",
                DVec3::new(-2.0, 0.0, 1.0),
            );
            crate::models::place(
                world,
                editor,
                "fetched/anim/characters/walker.glb",
                DVec3::new(1.0, 0.0, 2.0),
            );
        })),
        Step::Wait(20),
        Step::Shot("pie_1_edit"),
        Step::Do(Box::new(editor::play)),
        Step::Wait(90),
        Step::Shot("pie_2_playing"),
        Step::Do(Box::new(editor::stop)),
        Step::Wait(20),
        Step::Shot("pie_3_stopped"),
    ];
    ShotScript::new(dir, steps)
}

/// One brush dab at ground level under `at` (x, z).
fn sculpt(
    world: &mut World,
    editor: &mut Editor,
    at: DVec3,
    radius: f32,
    amount: f32,
    op: SculptOp,
) {
    let Some(landscape) = crate::landscape::find(world) else {
        return;
    };
    let ground = world.resource::<engine::world::Ground>().0.clone();
    let y = ground.sample(at).map_or(0.0, |s| s.height) + at.y;
    let dab = crate::landscape::Dab {
        at: [at.x, y, at.z],
        radius,
        amount,
        op,
    };
    crate::landscape::dab(world, editor, landscape, dab);
}
