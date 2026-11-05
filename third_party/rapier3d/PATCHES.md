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

## Considered, not applied

- Bumping `IslandManager::active_set_epoch` only when island membership changes (sleep, body
  type, colliders, enabled, dominance) instead of on any mutable access to a body. It cut full
  rebuilds of the solver contact graph from 10,884 to 997 in a run, but not time (5.4 -> 5.1 s):
  the rebuilds left are the expensive ones. Not worth carrying alone (round 27).
