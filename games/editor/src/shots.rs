//! Scripted headless runs that drive the editor like a person would and save screenshots of
//! the whole window: `editor --headless --size 1600x900 --shots <dir>`. Used for the gallery
//! (Tobias reviews the editor from these images) and as an end-to-end smoke test.

use std::collections::VecDeque;
use std::path::PathBuf;

use engine::prelude::*;

use crate::editor::{self, Create, Editor, Tab, Tool};
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
                    crate::prefab::place(world, editor, &path);
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
