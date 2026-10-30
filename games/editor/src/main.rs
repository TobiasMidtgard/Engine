//! The engine's editor.
//!
//! ```sh
//! cargo run -p editor --release                          # open the editor
//! cargo run -p editor --release -- --scene scenes/courtyard.scene.json
//! cargo run -p editor --release -- --headless --size 1600x900 --shots shots/
//! ```
//!
//! A scene view with an Unreal-style fly camera, an outliner, a details panel that edits any
//! registered component of any module, move/rotate/scale gizmos, undo and redo, scenes saved
//! as the engine's JSON scene documents, and play in editor (the world runs from a snapshot
//! and Stop puts the scene back). Every module's showcase demo can be placed as one object.
//!
//! The UI is egui, drawn as a render overlay on top of the scene (`egui_bridge`), and talks to
//! the world only through the type registry and the engine's contracts, like the AI bridge.

mod bookmarks;
mod browser;
mod camera;
mod camera_preview;
mod collision;
mod content;
mod convert;
mod curves;
mod demos;
mod editor;
mod egui_bridge;
mod folder;
mod graphs;
mod grips;
mod hide;
mod import;
mod inspector;
mod landscape;
mod material_edit;
mod materials;
mod matrix;
mod measure;
mod models;
mod outline;
mod partition;
mod picking;
mod prefab;
mod prefab_ui;
mod remap;
mod rewind;
mod rigs;
mod sample;
mod scatter;
mod scene_ops;
mod scripts;
mod sequencer;
mod shots;
mod show;
mod snap;
mod states;
mod story;
mod theme;
mod thumbs;
mod ui;
mod undo;
mod view;
mod viewport;

use engine::prelude::*;
use engine::render::RenderAppExt;

fn main() {
    // `editor --convert <model.fbx|.obj> <out.glb>`: the import converter on its own.
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--convert") {
        let (Some(src), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: editor --convert <model.fbx|model.obj> <out.glb>");
            std::process::exit(2);
        };
        match convert::to_glb(std::path::Path::new(src), std::path::Path::new(out)) {
            Ok(summary) => println!("{out}: {}", summary.describe()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let mut app = App::new();
    engine::core::log::init("info,wgpu=warn,naga=warn");
    {
        let mut config = app.world.resource_mut::<EngineConfig>();
        config.app_name = "Engine editor".into();
    }
    {
        let mut window = app.world.resource_mut::<Window>();
        window.title = "Engine editor".into();
    }
    engine::apply_cli_args(&mut app);

    app.add_plugins(DefaultPlugins)
        .add_plugins((
            engine_physics::PhysicsPlugin,
            engine_vehicles::VehiclesPlugin,
            engine_space::SpacePlugin,
            engine_terrain::TerrainPlugin,
            engine_water::WaterPlugin,
            engine_weather::WeatherPlugin,
            engine_fire::FirePlugin,
            engine_smoke::SmokePlugin,
        ))
        .add_plugins((
            engine_vfx::VfxPlugin,
            engine_foliage::FoliagePlugin,
            engine_anim::AnimationPlugin,
            engine_npc::NpcPlugin,
            engine_destruction::DestructionPlugin,
            engine_movement::MovementPlugin,
            engine_audio::AudioPlugin,
            engine_softbody::SoftBodyPlugin,
        ))
        .add_plugins((
            engine_script::ScriptPlugin,
            scripts::ScriptsTabPlugin,
            graphs::GraphsTabPlugin,
            rewind::RewindTabPlugin,
            curves::CurvesTabPlugin,
            sequencer::SequencerTabPlugin,
            story::StoryTabPlugin,
            grips::GripsTabPlugin,
        ))
        .add_plugins(EditorPlugin);

    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Some(path) = value("--scene") {
        app.insert_resource(StartScene(path));
    }
    if let Some(dir) = value("--shots") {
        let script = match value("--script").as_deref() {
            Some("round2") => shots::round2(dir),
            Some("round3") => shots::round3(dir),
            Some("round4") => shots::round4(dir),
            Some("round5") => shots::round5(dir),
            Some("round6") => shots::round6(dir),
            Some("round7") => shots::round7(dir),
            Some("round8") => shots::round8(dir),
            Some("round9") => shots::round9(dir),
            Some("round10") => shots::round10(dir),
            Some("round11") => shots::round11(dir),
            Some("round12") => shots::round12(dir),
            Some("round13") => shots::round13(dir),
            Some("round14") => shots::round14(dir),
            Some("round15") => shots::round15(dir),
            Some("round16") => shots::round16(dir),
            Some("round17") => shots::round17(dir),
            Some("round18") => shots::round18(dir),
            Some("round19") => shots::round19(dir),
            Some("round20") => shots::round20(dir),
            Some("round21") => shots::round21(dir),
            Some("round22") => shots::round22(dir),
            Some("round23") => shots::round23(dir),
            Some("round24") => shots::round24(dir),
            Some("round25") => shots::round25(dir),
            Some("round26") => shots::round26(dir),
            Some("round27") => shots::round27(dir),
            Some("round27b") => shots::round27b(dir),
            Some("play") => shots::play_in_editor(dir),
            Some("scripts") => scripts::shots(dir),
            Some("props") => scripts::props_shots(dir),
            Some("types") => scripts::types_shots(dir),
            Some("signals") => scripts::signals_shots(dir),
            Some("tests") => scripts::tests_shots(dir),
            Some("states") => scripts::states_shots(dir),
            Some("gizmos") => scripts::gizmos_shots(dir),
            Some("tools") => scripts::tools_shots(dir),
            Some("rewind") => scripts::rewind_shots(dir),
            Some("tweens") => scripts::tweens_shots(dir),
            Some("cutscenes") => scripts::cutscenes_shots(dir),
            Some("story") => scripts::story_shots(dir),
            Some("graphs") => graphs::shots(dir),
            Some("grips") => grips::shots(dir),
            Some("graph_signals") => graphs::signals_shots(dir),
            _ => shots::gallery(dir),
        };
        app.insert_resource(script);
    }
    app.run();
}

/// A scene file to open at start instead of the starter courtyard.
#[derive(Resource)]
struct StartScene(String);

/// Everything the editor adds to an app.
pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        let egui = egui_bridge::Egui::default();
        theme::apply(&egui.ctx);
        app.insert_resource(egui)
            .init_resource::<egui_bridge::EguiInput>()
            .init_resource::<egui_bridge::EguiFrame>()
            .init_resource::<ui::EditorDock>()
            .init_resource::<camera_preview::CameraPreview>()
            .register_component::<demos::ModuleDemo>(
                "Places one module's showcase demo here (module: physics, destruction, \
                 softbody, vehicles, fire, smoke, vfx, water, weather, foliage, terrain, anim, \
                 npc, movement, audio). The editor spawns it; the scene stores only this.",
            )
            .register_component::<landscape::Sculpt>(
                "A landscape's brush dabs (raise, lower, restore, paint), replayed onto its terrain.",
            )
            .register_component::<models::Model>(
                "Placed from the glTF file at `path` (relative to the asset root); its nodes are \
                 this entity's children, or a file with a skin plays here as a rig.",
            )
            .register_component::<rigs::ModelClips>(
                "Animation files (asset keys) whose clips this rigged model plays too, bones \
                 matched by name.",
            )
            .register_component::<scatter::ScatterGroup>(
                "Holds the copies of `model` painted with the scatter brush (one group per model).",
            )
            .register_component::<prefab::PrefabInstance>(
                "Placed from the prefab file at `path`; `overrides` and `removed` are what this \
                 copy changes, by part. Nested inside a prefab file: a nested prefab (at the \
                 root: the base of a variant).",
            )
            .register_component::<prefab::PrefabPart>(
                "Which part of its prefab file this entity is (its key there; \"\" for the root).",
            )
            .register_component::<folder::Folder>(
                "An outliner folder: sorts the scene, holds what is inside it, is not a thing in \
                 the world (the editor keeps its transform at identity).",
            )
            .add_systems(PostUpdate, hide::switch_off)
            .add_systems(Last, hide::switch_on)
            .add_render_feature(egui_bridge::EguiFeature::default())
            .add_render_feature(outline::OutlineFeature::default())
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (
                    egui_bridge::gather_input,
                    import::file_drops,
                    camera::editor_camera,
                    ui::editor_ui,
                    camera_preview::sync,
                    rigs::sync,
                    rigs::preview,
                    demos::sync_demos,
                    landscape::sync_sculpt,
                    models::watch_files,
                    prefab::track,
                    material_edit::flush,
                    step_frames,
                    shots::run_script,
                )
                    .chain(),
            );
    }
}

fn setup(world: &mut World) {
    let camera = editor::spawn_camera(
        world,
        DVec3::new(9.5, 6.0, 9.0),
        DVec3::new(-0.5, 0.6, -2.0),
    );
    let mut editor = editor::Editor::new(camera);
    let start = world.remove_resource::<StartScene>();
    match start {
        Some(StartScene(path)) => {
            world.insert_resource(editor);
            world.resource_scope(|world, mut editor: Mut<editor::Editor>| {
                editor::load_scene(world, &mut editor, &path);
            });
        }
        None => {
            scene_ops::load_scene(world, &sample::starter_scene());
            editor.info(
                0.0,
                "Starter courtyard loaded. Right mouse to fly, click to select.",
            );
            world.insert_resource(editor);
        }
    }
    // Edit mode: the world holds still until Play.
    world.resource_mut::<Time>().pause();
}

/// Advance one frame when Step is pressed while paused.
fn step_frames(
    mut editor: ResMut<editor::Editor>,
    mut time: ResMut<Time>,
    mut stepping: Local<bool>,
) {
    if *stepping {
        time.pause();
        *stepping = false;
    }
    if editor.step_once && editor.mode == editor::Mode::Paused {
        editor.step_once = false;
        time.resume();
        *stepping = true;
    }
}
