# Engine

Read [AGENTS.md](AGENTS.md) (conventions, build, units, how to write a module) and
[ARCHITECTURE.md](ARCHITECTURE.md) (crates and ownership) before changing code.

- Edit only the crate(s) you own; shared crates change through their owner (ARCHITECTURE.md).
- Active development (Tobias, 2026-10-20): commit on top of `origin/main`, run
  `scripts/quick-check`, and push straight to `main`; no pull request needed. Core crates and
  `engine_world` contracts still change through the lead. If your push turns main's CI red, fix
  it forward at once (AGENTS.md, "Build & test").
- Once per clone: `git config core.hooksPath .githooks`, so unformatted commits and pushes are
  refused instead of failing CI. Before pushing, run `cargo fmt --all` and clippy (AGENTS.md).
- Use `export CARGO_TARGET_DIR=$HOME/target` to share build artifacts.
- Headless rendering needs `mesa-vulkan-drivers` (lavapipe) when there is no GPU.
