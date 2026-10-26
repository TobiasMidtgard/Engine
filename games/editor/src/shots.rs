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
                    crate::prefab::place(world, editor, &path, None);
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
            crate::prefab::apply_changes(world, editor, post);
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
