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

mod camera;
mod demos;
mod editor;
mod egui_bridge;
mod inspector;
mod landscape;
mod models;
mod picking;
mod prefab;
mod sample;
mod scene_ops;
mod shots;
mod theme;
mod ui;
mod undo;
mod view;
mod viewport;

use engine::prelude::*;
use engine::render::RenderAppExt;

fn main() {
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
                 this entity's children.",
            )
            .register_component::<prefab::PrefabInstance>(
                "Placed from the prefab file at `path`; the editor can apply changes back to \
                 the file or revert to it.",
            )
            .add_render_feature(egui_bridge::EguiFeature::default())
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (
                    egui_bridge::gather_input,
                    camera::editor_camera,
                    ui::editor_ui,
                    demos::sync_demos,
                    landscape::sync_sculpt,
                    models::watch_files,
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
