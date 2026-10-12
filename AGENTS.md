# Working on the engine (humans and AI agents)

This file is the contract every contributor follows. `CLAUDE.md` points here.
Architecture and module ownership: [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Build & test

```sh
git config core.hooksPath .githooks   # once per clone: refuses commits `cargo fmt` would change
cargo build --workspace
cargo test -p <crate>                 # every crate has unit tests that need no GPU
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p hello                    # windowed
cargo run -p hello -- --headless --frames 4   # offscreen, writes screenshots/hello.png
```

Before every push, run `cargo fmt --all`, the clippy line above, and the tests of each crate
you changed and of every crate that uses what you changed (shared shaders, contracts and
assets reach other modules; `cargo test --workspace` covers all). CI runs all of it on every
pull request, and each failed run emails the repository owner.

Headless GPU tests and screenshots need a GPU or a software Vulkan driver. On Debian/Ubuntu
containers: `apt-get install -y mesa-vulkan-drivers` (lavapipe). Tests that need a GPU must
pass (with a note) when `RenderDisabled` is present, so CI without a GPU stays green.

Share one target directory between checkouts to avoid rebuilding dependencies:
`export CARGO_TARGET_DIR=$HOME/target`.

## Units and coordinates

- Right-handed, **Y up**, cameras and lights look along **−Z** (`Transform::forward()`).
- SI units everywhere: meters, seconds, kilograms, newtons, kelvin/°C (named `_c`), radians in
  code (degrees only in fields named `_deg`).
- Light in physical units: lux for the sun, lumens for lamps, cd/m² for emissive/ambient.
  Exposure is EV100. Never "fix" brightness with magic multipliers; fix units.
- **World positions are f64** (`Transform::translation: DVec3`, `GlobalTransform`), rotations
  and local offsets are f32. Convert near the point of use:
  `(world_pos - origin).as_vec3()`. Worlds span light-minutes; f32 world positions are a bug.
- The renderer draws everything relative to the camera (`ViewData::origin`). Shaders only
  ever see camera-relative positions (`rel`), plus `tiling_pos(rel)` for world-stable noise.

## Frame and time

```
First → PreUpdate → [FixedPreUpdate → FixedUpdate → FixedPostUpdate] × N → Update
      → PostUpdate → Extract → Render → Last
```

- Simulation that must be deterministic or stable (physics, fluids, fire spread, AI ticks,
  movement) runs in **FixedUpdate** in a `SimSet` (`Input → Gameplay → Physics → PostPhysics →
  Environment`). Presentation (cameras, UI, animation blending) runs in **Update**.
- `Res<Time>` gives the right delta in both: inside fixed schedules it is the fixed step.
- **Time dilation is built in.** `TimeDilation` slows or speeds the world (bullet time,
  "the enemy moves in slow motion"). Entities with `LocalTimeScale` run at their own rate
  (a player who perceives faster, an enemy that ignores slow-mo). Always scale per-entity
  motion with `time.delta_for(local_scale)`; never read real time for gameplay.
- Input for fixed-step systems: use `ActionState::pressed_since_tick`, not `just_pressed`.

## Making a module

A module is one crate (`crates/<name>`, package `engine_<name>`) with one `Plugin`.

1. **Own your crate.** Don't edit other modules' crates. Changes to `engine_core` or
   `engine_world` public APIs go through the core owner, and `engine_render` ones through the
   Rendering and lighting owner (open a PR or describe the change; see ARCHITECTURE.md).
2. **Talk through contracts, not crates.** A module never depends on another module's crate,
   so any module can be replaced (ARCHITECTURE.md, "Replacing a module"). Physics, animation,
   locomotion and terrain editing have contracts in `engine_world` (`physics::RigidBody`,
   `PhysicsQueries`, `animation::SkeletonPose`, `MoveIntent`, `TerrainEdit`, …); a physics
   backend must pass `engine_world::physics::conformance`. Everything else also goes through
   `engine_world`: messages (`WorldImpulse`, `ProjectileTrace`, `GroundContact`,
   `IgniteRequest`, `DamageEvent`, `NoiseEvent`), shared fields (`WindField`, `HeatField`,
   `SurfaceLayers`), queries (`Ground`, `Water`), `SurfaceMaterials`, `Gravity`. A smoke
   module reacts to `ProjectileTrace` without knowing which weapon fired. Use the `Env`
   system param to read the environment.
3. **Make it data-driven and AI-reachable.** Every component or resource a designer (or an
   AI agent) might tune derives `Serialize`/`Deserialize` with `#[serde(default)]` and is
   registered with a one-line doc:
   `app.register_component::<Wheel>("Suspension + tire for a vehicle wheel (meters, N/m).")`.
   Expose verbs as API functions named `<module>.<verb>` with JSON in/out:
   `app.register_api("fire.ignite", "Start a fire", r#"{"at": [0,0,0]}"#, |world, args| ...)`.
   The AI bridge, console and scripts call these; nothing else is needed to make a feature
   AI-controllable.
4. **Render through `RenderFeature`.** Implement the trait (prepare/draw/draw_shadow/
   draw_environment/overlay), add it with `app.add_render_feature(...)`. Compose shaders with
   `GpuShared::shader(label, src, with_frame)`; use `shade_surface`, `apply_fog`,
   `shadow_factor`, `wind_offset` from `lib_frame.wgsl` so lighting matches everything else.
   Output **pre-exposed** radiance (`pre_expose`). Add a unit test that runs
   `validate_wgsl(name, &compose_shader(src, true))`.
5. **Scale with the camera.** Simulations run at full detail near `SimulationFocus` entities
   and cheaper (or frozen) far away. Grids (`Grid2`/`Grid3`) recenter on focus.
6. **Tests.** Unit tests for logic (no GPU). For visuals, a headless test that renders and
   saves a PNG via `engine_render::capture_now` and checks something about the pixels.

## Art and large files

The bar is AAA: nothing in a screenshot or the showcase should look low poly or placeholder.
Get detail from procedural generation first (noise-shaped terrain and rocks, grown trees,
displaced and tessellated meshes, layered materials), and use CC0 scans (Poly Haven,
ambientCG) where scans beat procedural: rock, ground, bark, skies.

Storage (the first rule is a test, so CI enforces it):

- **No file over 1 MiB goes into git, and no third-party art at any size.** No Git LFS either:
  every clone and CI run of every thread would download it. `tools/fetch-assets`'s
  `no_large_files_are_committed` test fails the build otherwise.
- Large files are listed in `assets/manifests/<area>.toml` (one per thread, named after its
  area, so threads never conflict) and downloaded into `assets/fetched/` (gitignored) by
  `cargo run -p fetch-assets`. Code loads them with the asset key `fetched/<path>`.
- Every entry is pinned by SHA-256, links the `source` page, and has an allowed `license`:
  `CC0-1.0` preferred, CC-BY with an `author` to credit; nothing non-commercial or
  no-derivatives. Format and checks: `tools/fetch-assets` docs.
- Code must run when a fetched file is missing (CI and fresh checkouts don't fetch): fall back
  to a procedural or plain version and log once. Tests never depend on fetched files.

Adding a file:

1. Add the entry with `sha256 = ""` and push it to a scratch branch named `assets/<area>`
   (no pull request; CI doesn't run there). The Assets workflow downloads it, copies it to the
   `asset-mirror` release, and prints `sha256 = "…"` in its log and job summary. Never push an
   unpinned entry to a pull request: the test job fails until it is pinned.
2. Paste the pin into the manifest on your working branch, push that, and delete the scratch
   branch. From then on `cargo run -p fetch-assets` gets the file from the mirror even where
   the original host is blocked (as it is in the project's cloud sessions). Where the host is
   reachable, `cargo run -p fetch-assets` fetches unpinned entries and prints the pin itself.

Files made here (a model converted to GLB, a baked texture, an offline recording) have no
download link:

1. Give the entry `url = "mirror"`, a `source` linking what it was made from (or the code that
   made it), and that origin's `license` (`MIT OR Apache-2.0`, the engine's own, for art made
   from scratch). Put the file at `assets/fetched/<path>`.
2. Run `cargo run -p fetch-assets -- publish`. It uploads the file to the mirror and prints the
   pin. In the project's cloud sessions, where uploads are blocked, it pushes a short-lived
   `asset-upload/…` branch instead; the Asset upload workflow copies the files to the mirror and
   deletes the branch. Paste the pin and push once that workflow has run.

Fetching these needs a GitHub login (`gh auth login` or `GITHUB_TOKEN`) while the repository is
private, since only the mirror has them.

Sizes: 2K for most materials, 4K for hero assets and terrain close-ups, 2K–4K HDRIs for
lighting (8K only for a visible backdrop). JPG or PNG for color, HDR or EXR only for HDR data.

## Performance rules

- No allocation per entity per frame in hot paths. Keep scratch `Vec`s in the system's
  `Local` or the feature struct and `clear()` them.
- Cache `QueryState` in render features; use `par_iter_mut` for large ECS loops and
  `engine_core::tasks::par_chunks_mut` for grids.
- GPU: instance everything; one storage buffer per kind of data; avoid per-object bind
  groups; compute shaders for anything with more than ~10k elements (particles, fluids).
- Measure: `Diagnostics::set("module.metric", v)` shows up in `engine.describe`.

## Style

- `cargo fmt`, clippy clean, Rust 2024 edition.
- Doc comment on every public item, written for someone (or an AI) using it for the first time,
  with units.
- Errors returned to API callers are plain sentences that say what to do.
- Commit messages: imperative subject, body explains why.
