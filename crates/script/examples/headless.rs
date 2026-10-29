//! Scripts without the game: runs Luau scripts on entities of their own at 30 frames a second,
//! with the script debugger listening, so VS Code (or another DAP client) can attach and stop
//! in them without starting the game or a GPU.
//!
//! ```sh
//! cargo run -p engine_script --example headless                      # the Script courtyard
//! cargo run -p engine_script --example headless -- examples/float.luau examples/lamp.luau
//! cargo run -p engine_script --example headless -- --folder /tmp/scripts --port 4711 a.luau
//! ```
//!
//! Each file runs on an entity named after it (`float.luau` on "Float"). Prints and errors go
//! to the terminal. Each has a mesh and a light, as plain components (nothing is drawn).

use std::time::{Duration, Instant};

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::{MeshRenderer, PointLight, Shape, StandardMaterial};
use engine_script::{Script, ScriptPlugin, ScriptRuntime, ScriptSettings};
use serde_json::Value;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut folder = None;
    let mut port = 4711u16;
    let mut files = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--folder" => folder = args.next(),
            "--port" => port = args.next().and_then(|p| p.parse().ok()).unwrap_or(port),
            _ => files.push(a),
        }
    }
    engine_core::log::init("info");
    let mut app = App::new();
    let mut settings = ScriptSettings {
        debug_port: Some(port),
        ..Default::default()
    };
    if let Some(folder) = folder {
        settings.folder = folder;
    }
    app.insert_resource(settings);
    app.insert_resource(Assets::<StandardMaterial>::default());
    app.register_component::<MeshRenderer>("A mesh (not drawn here).")
        .register_component::<PointLight>("A light (not drawn here).")
        .register_component::<engine_world::Interactable>("Usable with E.")
        .add_message::<engine_world::Interact>();
    // The effects the showcase scripts call do nothing here.
    for name in ["vfx.spawn", "vfx.beam"] {
        app.register_api(name, "Does nothing without the game.", "{}", |_, _| {
            Ok(Value::Null)
        });
    }
    app.add_plugins(ScriptPlugin);
    if files.is_empty() {
        let mut queue = bevy_ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &app.world);
        engine_script::demo::spawn_demo(&mut commands, DVec3::new(70.0, 0.0, -105.0));
        queue.apply(&mut app.world);
    }
    for file in &files {
        let stem = file
            .rsplit('/')
            .next()
            .unwrap_or(file)
            .trim_end_matches(".luau");
        let mut name: String = stem.replace('_', " ");
        if let Some(first) = name.get(0..1) {
            name = first.to_uppercase() + &name[1..];
        }
        app.world.spawn((
            Name::new(name),
            Transform::from_translation(DVec3::new(0.0, 0.0, 0.0)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.2,
                    segments: 16,
                },
                StandardMaterial::default(),
            ),
            PointLight::default(),
            Script::new(file.clone()),
        ));
    }
    println!(
        "Running {}. Attach a debugger to localhost:{port} (VS Code: \"Attach to the game's \
         scripts\"); Ctrl+C stops.",
        if files.is_empty() {
            "the Script courtyard".to_string()
        } else {
            files.join(", ")
        }
    );
    let frame = Duration::from_secs_f64(1.0 / 30.0);
    let mut cursor = engine_script::editing::OutputCursor::default();
    loop {
        let start = Instant::now();
        app.update();
        if let Some(rt) = app.world.get_resource::<ScriptRuntime>() {
            for line in rt.output_since(&mut cursor) {
                let error = if line.error { "error: " } else { "" };
                println!("{error}{}", line.text);
            }
        }
        if let Some(rest) = frame.checked_sub(start.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}
