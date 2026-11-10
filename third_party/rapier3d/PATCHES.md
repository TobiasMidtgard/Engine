# Engine patches to Rapier 0.36.1

This folder is `rapier3d` 0.36.1 exactly as published on crates.io (unpacked from
`rapier3d-0.36.1.crate`, upstream commit 475caa3f, `crates/rapier3d`), plus `LICENSE` (the
Apache-2.0 text the crate's `Cargo.toml` names; the published crate carries no copy) and the
changes below, each its own commit on top. The root `Cargo.toml` swaps it in with
`[patch.crates-io]`; the folder is outside the workspace, so it is not formatted, linted or
tested with it.

To go back to stock Rapier: delete the `[patch.crates-io]` entry in the root `Cargo.toml`, this
folder and the `exclude` line, and run `cargo update -p rapier3d`.

## 1. Cap the island searches a step runs (`src/dynamics/island_manager/local_split.rs`)

- **What**: `PersistentIslands::resolve_removals` runs at most `MAX_SEARCHES_PER_STEP` (32)
  connectivity searches a step. The step's other removals are handed to the global split
  (`constraint_remove_count += 1`), exactly as a search that runs over `SEARCH_BUDGET` is.
- **Why**: for every contact lost between two bodies of one awake island that are not both
  moving, Rapier runs a dual search (up to 1,024 expansions) to prove the island is still
  connected. Resting contacts in a big pile flicker on and off, so a settling pile loses
  hundreds of them a step. Measured on the proving ground (`yard_arrival`, focus 20 m from the
  yard, VM, 20 s): 75,700 removals, up to 1,872 in one step, 22,375 searched, 93% of them
  "still connected"; 0.9 s of searching, and the worst step 18-23 ms of it. Rapier's profiler
  reports this as "islands". The lead's release route logged a 196.6 ms step with "islands
  105.9" at the village (engine-design/physics-perf-research.md, round 29).
- **Gain**: same run, 0.88 -> 0.33 s of searching, worst step 22.8 -> 7.5 ms (local copy,
  2026-11-05). On this vendored copy, `PHYSICS_REPORT=1 YARD_ARRIVAL_FOCUS_M=20 yard_arrival 20`,
  3 runs each, the cap against `usize::MAX`: Rapier's "islands" time over the logged steps
  815-923 -> 321-336 ms, the worst step's 15.9-20.3 -> 7.9-11.6 ms. Whole steps barely move on
  that overloaded 4-core VM (mean 35.1-35.8 -> 33.4-34.1 ms over the logged steps).
- **Cost**: a capped removal that did split an island waits for the global split, which
  splits one island a step, so a body that came apart from a pile may be woken or put to sleep
  with its old island for a few more steps. `crates/physics/tests/behaviors.rs`
  (`a_pile_sleeps_wakes_and_sleeps_again_with_the_search_cap`) checks a pile still falls
  asleep, wakes when struck and sleeps again.
- **Drop it when**: upstream bounds this work per step (or skips searches for removals between
  resting bodies). Write-up: engine-design/physics-perf-research.md, end (local draft, not filed).

## 2. Bump the active-set epoch only when the active set can have changed (`src/dynamics/island_manager/manager.rs`)

- **What**: `IslandManager::rigid_body_updated` bumps `active_set_epoch` only when the body's
  type, enabled or sleeping state, island or slot in the awake list changed, or its change flags
  hold `TYPE`, `SLEEP`, `ENABLED_OR_DISABLED`, `DOMINANCE` or `COLLIDERS`. Stock Rapier bumps it
  for every body the user touched.
- **Why**: the epoch keys the solver contact graph's full rebuild
  (`NarrowPhase::maintain_solver_contact_graph`), a walk over every contact pair of the world,
  sleeping ones included. The engine sets forces, velocities, damping or kinematic targets on a
  few awake bodies every substep (cars, characters, time-scaled bodies, buoyancy), so nearly
  every substep rebuilt: on the proving ground bench (`crates/physics/examples/pg_bench.rs`)
  15,809 of 16,000 maintenance calls were full rebuilds, all from the epoch.
- **Gain**: bench, 1,274 of 16,000 full rebuilds. Showcase physics stop (VM, `--loops physics:3
  --zones all`, rounds 2-3): Rapier's whole step 4.63 -> 3.12 ms, its constraint collection
  2.86 -> 1.71, the fixed step 20.9-24.4 -> 16.4 ms a run, `physics_simulate` 25.6-35.4 ->
  17.7-18.0 ms a frame.
- **Cost**: none known: the epoch's other users (the body qualification table, the joint
  selection memo, the staged joint layout) depend only on the same active-set membership, and
  Rapier's debug validators (`debug_validate_body_qualify_info`, `debug_validate_solver_graph`)
  check the cached state against a rebuild every step in debug builds.
  `crates/physics/tests/behaviors.rs`
  (`a_pushed_row_keeps_its_contacts_when_bodies_change_without_leaving_the_active_set`) pushes a
  row of boxes with a force every step, turns one kinematic and back, and lets the row sleep.
- **Drop it when**: upstream bumps the epoch only on membership changes. (Round 27 tried this
  alone and measured little; with today's pile of 43,000 pairs it is the larger part of the
  collection stage.)

## 3. Rebuild the solver contact graph over the awake pairs only (`src/geometry/narrow_phase/solver_graph.rs`)

- **What**: when the solver contact graph must be rebuilt (an epoch bump: a body fell asleep,
  woke or changed type; a body falling asleep; a composite pair rebuilding its manifolds) and no
  contact-graph edge index has moved since the last rebuild, `maintain_solver_contact_graph`
  walks only the pairs the graph holds now and the pairs of the awake bodies' colliders
  (`NarrowPhase::sparse_rebuild_edges`), in ascending order, instead of every pair of the world.
  `NarrowPhase::solver_graph_edges_stable` says whether edge indices held still: collider node
  removal (which swap-removes edges) and degenerate bookkeeping clear it, every rebuild sets it.
  When those pairs are over a quarter of all pairs, or edges moved, or the multibody topology
  changed, it walks everything as before. `SolverContactGraph::clear` is now in parallel builds
  too.
- **Why**: a manifold can only be solver-active with an awake dynamic body
  (`qualify_manifold_bqi`), and every manifold outside the graph already has no graph position,
  so the pairs between sleeping or static bodies, most of a big settled world, need no visit.
  After patch 2 the proving ground's pile still rebuilt on about 8% of substeps (bodies falling
  asleep and waking, composite pairs), each over 43,000-49,000 pairs.
- **Gain**: proving ground bench (`pg_bench 900`, VM, release): 3.84-4.59 ms a step stock,
  3.54-3.93 with patch 2, 2.39-2.44 with patches 2 and 3. In a debug run of 3,000 steps, 2,488
  rebuilds went sparse (about 590 pairs each instead of about 48,000) and 713 stayed full.
  Showcase physics stop (VM, as in 2): Rapier's whole step 3.12 -> 1.4-2.1 ms, its constraint
  collection 1.71 -> 0.12-0.18; the physics step's own work (sync, upkeep, plans, substeps,
  characters, write back) 9.1 ms stock -> 6.3 with patch 2 -> 4.3 with both.
- **Cost**: the layout is the full walk's (ascending pairs, so a parallel and a serial build
  still agree). Rapier's debug validator (`debug_validate_solver_graph`, every step in debug
  builds) found nothing over that 3,000-step debug run of the proving ground or the engine's
  physics tests.
- **Drop it when**: upstream rebuilds only the awake part of the graph (or stops rebuilding on
  sleep and wake).

## 4. Keep joints between disabled bodies out of the solver (`src/dynamics/joint/impulse_joint/impulse_joint_set.rs`)

- **What**: `ImpulseJointSet::select_active_interactions` counts a disabled dynamic or
  kinematic body as world-attached, like a fixed one, and skips a joint whose two sides are
  both world-attached.
- **Why**: a disabled body is out of the active set, so its solver id is `u32::MAX`, but the
  selection only looked at its type and sleep state. A joint between two disabled bodies then
  reached the joint coloring with no solver body on either side and hit its `unreachable!()`
  (`ParallelInteractionGroups::group_interactions`, "internal error: entered unreachable code").
  Rapier disables a joint itself when a body it holds is disabled, but not one made later:
  in the Weaving hall's blades stop a ragdoll went non-finite, Rapier's quarantine switched
  every part off, the engine moved the parts to another region and made their joints again
  there, enabled, and the next step crashed (2026-11-10).
- **Gain**: no crash. A joint between a disabled body and a moving one still holds the moving
  one, with the disabled side fixed in place.
- **Cost**: none.
- **Test**: `crates/physics/tests/quarantine.rs` (both tests panic without the patch).
- **Drop it when**: upstream checks `is_enabled` in the joint selection.
