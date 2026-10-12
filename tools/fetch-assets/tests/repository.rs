//! Repository rules that `cargo test` enforces on every PR: manifests are valid and pinned,
//! and no large file is committed to git.

use fetch_assets::*;

#[test]
fn asset_manifests_are_valid_and_pinned() {
    let root = workspace_root();
    let manifests = load_manifests(&root).expect("manifests parse");
    let problems = validate(&manifests, true);
    assert!(
        problems.is_empty(),
        "fix assets/manifests (see AGENTS.md, \"Art and large files\"):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn no_large_files_are_committed() {
    let root = workspace_root();
    let Some(big) = oversized_tracked_files(&root) else {
        eprintln!("git is not available here; skipping the large file check");
        return;
    };
    assert!(
        big.is_empty(),
        "files over {} KiB don't go in git; list them in assets/manifests instead \
         (see AGENTS.md, \"Art and large files\"):\n  {}",
        MAX_COMMITTED_BYTES / 1024,
        big.iter()
            .map(|(name, len)| format!("{name} ({} KiB)", len / 1024))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
