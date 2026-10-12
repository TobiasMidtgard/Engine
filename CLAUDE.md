# Engine

Read [AGENTS.md](AGENTS.md) (conventions, build, units, how to write a module) and
[ARCHITECTURE.md](ARCHITECTURE.md) (crates and ownership) before changing code.

- Edit only the crate(s) you own; shared crates change through the core owner.
- Use `export CARGO_TARGET_DIR=$HOME/target` to share build artifacts.
- Headless rendering needs `mesa-vulkan-drivers` (lavapipe) when there is no GPU.
