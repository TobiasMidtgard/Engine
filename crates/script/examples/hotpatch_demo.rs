//! Rust hot patching, the smallest case: run this under the Dioxus CLI, edit `greeting` or
//! `report` below while it runs, save, and the printed line changes without a restart.
//!
//! ```sh
//! cargo install dioxus-cli@0.7.10 --locked     # once
//! dx serve --hot-patch -p engine_script --example hotpatch_demo --features hotpatch
//! ```
//!
//! Without `dx` (or the feature) it is a plain program that prints once a second.

use engine_core::prelude::*;

#[derive(Resource, Default)]
struct Frames(u64);

fn greeting(frame: u64) -> String {
    format!("frame {frame}: the first version of greeting()")
}

fn report(mut frames: ResMut<Frames>) {
    frames.0 += 1;
    if frames.0.is_multiple_of(30) {
        println!("{}", greeting(frames.0));
    }
}

fn main() {
    let mut app = App::new();
    app.init_resource::<Frames>()
        .add_plugins(engine_script::ScriptPlugin)
        .add_systems(Update, report);
    let frames = std::env::args()
        .nth(1)
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(u64::MAX);
    for _ in 0..frames {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(33));
    }
}
