# Working on the engine (humans and AI agents)

This file is the contract every contributor follows. `CLAUDE.md` points here.
Architecture and module ownership: [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Build & test

```sh
git config core.hooksPath .githooks   # once per clone: refuses commits and pushes `cargo fmt` would change
cargo build --workspace
cargo test -p <crate>                 # every crate has unit tests that need no GPU
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p hello                    # windowed
cargo run -p hello -- --headless --frames 4   # offscreen, writes screenshots/hello.png
```

Before every push, run `scripts/quick-check`. It checks formatting across the workspace, lints
the crates your change touches (the whole workspace when a core crate, `engine_world` or the
Cargo files change) and runs their unit tests (`--lib`), in a few minutes. `scripts/quick-check
--full` also runs their integration and render tests; use it when you change shared shaders,
contracts or assets that reach other modules.

While the engine is in active development (Tobias, 2026-10-20), work lands by pushing straight
to `main` after the quick check: `git fetch origin main && git rebase origin/main`, then
`scripts/quick-check`, then `git push origin HEAD:main` (fetch and rebase again if the push is
refused because main moved). CI runs every test on each push to `main` and each failed run
emails the repository owner, so whoever's push turned main red fixes it forward right away,
before anything else. Core crates and `engine_world` contracts still change through the lead
(ARCHITECTURE.md).

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
- **The world re-anchors** (`WorldShift`: floating origin, planet frames, `showcase.goto`).
  Root `Transform`s and the contracts move by themselves; any world point you keep yourself
  (a demo's path points, pivots, targets, homes; a cached goal; a grid origin) must follow:
  `impl FollowsWorldShift` and `app.shift_components::<C>()` / `shift_resource::<R>()`, or
  read `WorldShift` in `WorldShiftSet::Apply`. Test it with one shifted update. A hoist that
  didn't follow lowered the yard's cloth into its statue after a jump (16 ms of
  self-collision on the crushed sheet).
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
4. **Render through `RenderFeature`.** Implement the trait (prepare/draw/draw_prepass/
   draw_shadow/draw_environment/overlay), add it with `app.add_render_feature(...)`. Opaque
   geometry also implements `draw_prepass` so ambient occlusion and anti-aliasing see it. Compose shaders with
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
  `CC0-1.0` preferred, CC-BY with an `author` to credit, MIT, Apache-2.0 or OFL-1.1 with the
  `copyright` line from the origin's license file; nothing non-commercial or no-derivatives
  (most motion-capture datasets, such as LAFAN1 and Bandai Namco's, are CC BY-NC-ND, so they
  are out). Format and checks: `tools/fetch-assets` docs.
- The repository and the mirror are public, so licenses travel with the files: fetching
  writes `assets/fetched/THIRD_PARTY_NOTICES.md` (every file's license, author and source,
  CC-BY credits, and the license texts), the Assets workflow puts the same file on the mirror,
  and a build that ships fetched files ships it too.
- Code must run when a fetched file is missing (CI and fresh checkouts don't fetch): fall back
  to a procedural or plain version and log once. Tests never depend on fetched files.

Adding a file:

1. Put the entry, with `sha256 = ""`, on a fresh scratch branch from main and push it (no pull
   request; CI doesn't run there, so nothing turns red):

   ```sh
   git fetch origin main
   git switch -c assets/<area> origin/main    # from main, so it has the current workflow
   # add the entry to assets/manifests/<area>.toml, then commit only that file
   git push origin assets/<area>    # --force if an old assets/<area> branch is still there
   ```

   The Assets workflow downloads the file, copies it to the `asset-mirror` release, reports
   `sha256 = "…"` as an annotation on its check run, and deletes the scratch branch.
2. Read the pin (the run takes a few minutes; `status` turns `completed`):

   ```sh
   sha=$(git rev-parse HEAD)    # the commit you pushed
   run=$(gh api repos/TobiasMidtgard/Engine/commits/$sha/check-runs \
     --jq '.check_runs[] | select(.name == "Mirror fetched assets") | .id')
   gh api repos/TobiasMidtgard/Engine/check-runs/$run/annotations \
     --jq '.[] | select(.title | startswith("Asset")) | .message'    # pins, or what failed
   ```

   Paste the pin into the manifest on your working branch and push that. If the run failed
   (a bad URL, say), fix the entry, commit and push the scratch branch again. Never push an
   unpinned entry to a pull request: the test job fails until it is pinned.

From then on `cargo run -p fetch-assets` gets the file from the mirror even where the original
host is blocked (as it is in the project's cloud sessions). Where the host is reachable,
`cargo run -p fetch-assets` fetches unpinned entries and prints the pin itself.

Files made here (a model converted to GLB, a baked texture, an offline recording) have no
download link:

1. Give the entry `url = "mirror"`, a `source` linking what it was made from (or the code that
   made it), and that origin's `license` (`MIT OR Apache-2.0`, the engine's own, for art made
   from scratch). Put the file at `assets/fetched/<path>`.
2. Run `cargo run -p fetch-assets -- publish`. It uploads the file to the mirror and prints the
   pin. In the project's cloud sessions, where uploads are blocked, it pushes a short-lived
   `asset-upload/…` branch instead; the Asset upload workflow copies the files to the mirror and
   deletes the branch. Paste the pin and push once that workflow has run, which is when
   `git ls-remote origin refs/heads/asset-upload/<id>` prints nothing.

Only the mirror has these. Fetching them needs no login now that the repository is public (a
`GITHUB_TOKEN` or `gh auth login` raises GitHub's rate limit).

Sizes: 2K for most materials, 4K for hero assets and terrain close-ups, 2K–4K HDRIs for
lighting (8K only for a visible backdrop). JPG or PNG for color, HDR or EXR only for HDR data.

## Performance rules

- No allocation per entity per frame in hot paths. Keep scratch `Vec`s in the system's
  `Local` or the feature struct and `clear()` them.
- Cache `QueryState` in render features; use `par_iter_mut` for large ECS loops and
  `engine_core::tasks::par_chunks_mut` for grids.
- Never hold a lock guard (`Mutex`, `RwLock`, a shared state's `write()`/`read()`) across a
  task pool `scope`, `par_iter` or `par_chunks_mut`. While it waits, the thread runs other
  modules' tasks, and one that takes the same lock (for example a `Water` or `Ground` sample)
  deadlocks the frame. Round 9 on the PC froze this way: water's flow step held its state's
  write lock across `par_bands`, and its thread picked up a fire task that read `Water`.
  Copy or take what the parallel part needs, drop the guard, then go wide.
- Never open a task pool `scope` inside a task already running on that pool. The waiting
  thread ticks the pool and runs unrelated tasks, so the outer task stretches by theirs
  (Softbody measured an 806 ms stall). Turning that ticking off can deadlock when every worker
  waits. Flatten the work into one scope opened from a system, or give leaf work that never
  waits a small pool of its own. Slow one-off builds go on `AsyncComputeTaskPool`.
- Let systems run side by side. A profile in round 19 had ~90% of the frame running one
  system at a time: the executor only overlaps systems whose data doesn't clash, and most
  clash. `ENGINE_PROFILE=1` prints "one system at a time / no system / several at once"
  and a "ran alone" list every report, and once, at frame 30, a "schedule parallelism" list
  of each schedule's exclusive systems and the data most systems fight over. Fix what is
  heavy in "ran alone" first; a cheap system that runs alone costs little. So:
  - A `&mut World` system runs with nothing beside it, and the executor can only start it
    (or skip it on its `run_if`) once nothing else is running, and systems ordered after it
    wait for that too. Keep heavy work out of them (take the queries you need in a normal
    system), and order nothing after one you gate.
  - Keep `&mut Transform` (and `GlobalTransform`, `MeshRenderer`, `Assets<Mesh>`) out of
    your heavy systems: 79 systems write `Transform` in `FixedUpdate` and 42 in `Update`,
    and any two of them clash. A marker doesn't help: `With<A>` and `With<B>` still clash in
    bevy_ecs 0.20; only `With<A>` against `Without<A>` is disjoint. Do the work in a system
    that reads (`&Transform`) and writes your own component, then copy the result into
    `Transform` in a small system after it, as Unity's DOTS jobs do. Read with `&Transform`
    whenever you only read.
  - A `Commands` user ordered before another system gets an `apply_deferred` sync point
    after it, which waits for everything running. Spawn and despawn in batches, not every
    frame, and prefer mutating existing components.
  - `.chain()` only systems that truly need an order; a chain of 10 is 10 serial steps.
  - Split one system that writes several unrelated resources into one per resource.
  - `FixedUpdate`'s `SimSet`s run one after another, so nothing overlaps the physics step.
    A system that doesn't need this step's bodies (no raycasts, overlaps or body poses; a
    wind, cloud, smoke or water-flow step on last step's state) goes in
    `SimSet::DuringPhysics`, which runs beside `Physics`, like Unreal's `TG_DuringPhysics`.
- GPU: instance everything; one storage buffer per kind of data; avoid per-object bind
  groups; compute shaders for anything with more than ~10k elements (particles, fluids).
- Measure: `Diagnostics::set("module.metric", v)` shows up in `engine.describe`.

## Style

- `cargo fmt`, clippy clean, Rust 2024 edition.
- Doc comment on every public item, written for someone (or an AI) using it for the first time,
  with units.
- Errors returned to API callers are plain sentences that say what to do.
- Never seed randomness or pick variations from `Entity::to_bits()` or other entity ids. In
  bevy_ecs 0.20 every resource and message queue is an entity too, so a new resource or
  message anywhere renumbers everything spawned after it and changes looks and behaviour
  (an NPC revive test moved from 5.8 s to 6.6 s when engine_world gained one message). Store a
  seed on the entity when it is spawned (like `BodyRequest::seed`) and use that.
- Don't let the order a query visits entities in decide behaviour ("the first eight to wake
  think now"): it follows archetype creation, so any unrelated archetype or resource made
  earlier changes it. Count first, then decide per entity from its own seed.
- Asset handles carry generations: one kept past `Assets::remove` reads as missing. A demo
  whose streamed cell frees its assets (`StreamCell::release_assets`) keeps mesh data, not
  handles, between loads.
- A level that is one big scene document streams by itself: `engine::scene_stream::stream_scene`
  (or `scene.stream`) cuts its roots into 128 m runtime-grid cells by position, the way Unreal's
  World Partition does; mark sky, settings and global lights `StreamPlacement { spatial: false }`
  and give big things their `radius`. Don't cut levels into cells by hand.
- Streamed content spawns where its cell is now: a cell's center follows world shifts, so place
  things from `StreamedCells` (or a module's shifted anchor), never from coordinates fixed at
  startup. A cell that `remember()`s must spawn the same entities with the same names and places
  every time; a module whose state matters across unloads adds its component with
  `CellMemory::remember("Name")`.
- Generated content that is too big to register up front (a planet's surface details, a belt,
  a star system) streams through a procedural grid: `world.stream_grid(StreamGrid::new(name,
  cell_m, load_m), maker)`; the maker decides per `GridCell` from its `seed()` (`None` = empty)
  and spawns from the `CellPlace` it is handed. Space uses `.vertical()` cube cells and
  `.on_frame(body)`; content that rides the body is a child of `place.frame` at
  `place.in_frame(..)`. Cells retire behind the sources and come back with the same key.
- Commit messages: imperative subject, body explains why.
