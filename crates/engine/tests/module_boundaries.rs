//! Guards the rule in ARCHITECTURE.md that makes modules replaceable: a system module crate
//! depends only on core crates and reaches other modules through `engine_world` contracts.
//!
//! The check reads the workspace as Cargo resolves it (`cargo metadata`), not the manifest text,
//! so every way of writing a dependency counts: `[dependencies.x]` tables, renames
//! (`phys = { package = "engine_physics", ... }`), quoted keys, dev-, build- and
//! target-specific dependencies, and dependencies on the `engine` facade or on games and tools.

use std::collections::HashSet;
use std::path::{Component, Path};
use std::process::Command;

use serde_json::Value;

/// Crates owned by the core, by directory name under `crates/`. Everything else under `crates/`
/// is a system module.
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

/// The workspace packages a module may depend on.
const ALLOWED: &[&str] = &[
    "engine_core",
    "engine_world",
    "engine_render",
    "engine_assets",
    "engine_input",
    "engine_debug",
];

/// `cargo metadata` for the whole workspace (workspace packages only).
fn workspace_metadata() -> Value {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .unwrap_or_else(|e| panic!("could not run `cargo metadata`: {e}"));
    assert!(
        output.status.success(),
        "`cargo metadata --manifest-path {}` failed:\n{}",
        manifest.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("`cargo metadata` printed invalid JSON")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("`cargo metadata` output has no string `{key}`: {value}"))
}

#[test]
fn modules_depend_only_on_core_crates() {
    let metadata = workspace_metadata();
    let crates_dir = Path::new(text(&metadata, "workspace_root")).join("crates");
    let packages = metadata["packages"]
        .as_array()
        .expect("`cargo metadata` output has no `packages`");
    let workspace: HashSet<&str> = packages.iter().map(|p| text(p, "name")).collect();

    let mut modules = 0;
    let mut violations = Vec::new();
    for package in packages {
        let manifest = Path::new(text(package, "manifest_path"));
        // The directory directly under `crates/` this package lives in.
        let Some(Component::Normal(dir)) = manifest
            .strip_prefix(&crates_dir)
            .ok()
            .and_then(|rel| rel.components().next())
        else {
            continue; // games/, tools/: not modules
        };
        let dir = dir.to_string_lossy();
        if CORE.contains(&dir.as_ref()) {
            continue;
        }
        modules += 1;
        let deps = package["dependencies"].as_array().into_iter().flatten();
        for dep in deps {
            // `name` is the real package name, also when the dependency is renamed.
            let name = text(dep, "name");
            if !workspace.contains(name) || ALLOWED.contains(&name) {
                continue;
            }
            let mut how = Vec::new();
            if let Some(kind) = dep["kind"].as_str() {
                how.push(format!("{kind}-dependency"));
            }
            if let Some(rename) = dep["rename"].as_str() {
                how.push(format!("renamed to `{rename}`"));
            }
            if let Some(target) = dep["target"].as_str() {
                how.push(format!("for target `{target}`"));
            }
            let how = if how.is_empty() {
                String::new()
            } else {
                format!(" ({})", how.join(", "))
            };
            violations.push(format!(
                "crates/{dir} ({}) depends on {name}{how}",
                text(package, "name")
            ));
        }
    }
    assert!(
        modules > 0,
        "found no module crates under {}; the workspace layout changed, update this test",
        crates_dir.display()
    );
    assert!(
        violations.is_empty(),
        "module crates must depend only on {} and never on other modules, the `engine` facade, \
         games or tools; add a contract to engine_world instead (see ARCHITECTURE.md):\n  {}",
        ALLOWED.join(", "),
        violations.join("\n  ")
    );
}
