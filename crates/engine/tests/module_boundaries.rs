//! Guards the rule in ARCHITECTURE.md that makes modules replaceable: a system module crate
//! depends only on core crates and reaches other modules through `engine_world` contracts.

use std::fs;
use std::path::Path;

/// Crates owned by the core. Everything else under `crates/` is a system module.
const CORE: &[&str] = &[
    "core",
    "world",
    "render",
    "assets",
    "input",
    "platform",
    "engine",
    "ai_bridge",
    "debug",
];

/// What a module may depend on.
const ALLOWED: &[&str] = &[
    "engine_core",
    "engine_world",
    "engine_render",
    "engine_assets",
    "engine_input",
];

#[test]
fn modules_depend_only_on_core_crates() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut violations = Vec::new();
    for entry in fs::read_dir(&crates).unwrap() {
        let dir = entry.unwrap().path();
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let Ok(manifest) = fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        if CORE.contains(&name.as_str()) {
            continue;
        }
        for line in manifest.lines() {
            let dep = line
                .split(['=', '.', ' '])
                .next()
                .unwrap_or_default()
                .trim();
            if dep.starts_with("engine_") && !ALLOWED.contains(&dep) {
                violations.push(format!("crates/{name} depends on {dep}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "module crates must not depend on other modules; add a contract to engine_world \
         instead (see ARCHITECTURE.md):\n  {}",
        violations.join("\n  ")
    );
}
