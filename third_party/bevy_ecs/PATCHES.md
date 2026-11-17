# Engine patches to bevy_ecs 0.20.0

This folder is `bevy_ecs` 0.20.0 exactly as published on crates.io (`Cargo.toml`,
`Cargo.toml.orig`, `src`, `examples`, both licence files, `README.md`), plus the changes below,
each its own commit on top. The root `Cargo.toml` swaps it in with `[patch.crates-io]`; the
folder is outside the workspace, so it is not formatted, linted or tested with it.

To go back to stock bevy_ecs: delete the `bevy_ecs` line under `[patch.crates-io]` in the root
`Cargo.toml` and this folder, and run `cargo update -p bevy_ecs`.

## 1. Cheap systems run inline in the multi-threaded executor (`src/schedule/executor/multi_threaded.rs`)

- **What**: the executor keeps, per system, a decaying maximum of its run time (a longer run
  sets it, shorter ones bring it down an eighth of the gap per run). A `Send`, non-exclusive
  system whose estimate is under 20 µs is not spawned as a pool task: once the pass over the
  ready systems has spawned the others, it runs on the thread that is ticking the executor,
  and its dependents are released at once. `ENGINE_INLINE_SYSTEMS_US` sets the threshold in
  microseconds; `0` turns it off (stock behaviour). A system's first run is always a task.
- **Why**: the showcase runs about 1,000 systems a frame, and at the plaza 9 in 10 of them find
  nothing to do and take under 5 µs. Each still went to a pool thread and back (a task spawn,
  a thread wake, the completion queue and the next executor tick): 3.3 µs a system on the
  cloud machine against 0.35 µs on the single-threaded executor, so most of the fixed step
  and Update was hand-offs (cloud plaza trace, 17 Nov: FixedUpdate 4.6 ms a step of which
  systems 1.5 ms over 362 runs; Update 3.5 ms of which 1.7 over 408). The single-threaded
  executor is no answer: it loses the overlap of the heavy systems (round 27 on the PC).
- **Safety**: an inline system is marked running before the next ready system is checked, so
  `can_run` keeps it apart from anything conflicting exactly as for a task; it runs with the
  same `run_unsafe` and `handle_errors`, and a panic is stored for the end of the run as a
  task's is. Non-`Send` and exclusive systems are never inlined.
- **Measured**: `crates/core` bench of 400 idle systems (half writing one component, half
  reading another), cloud machine: 1,333 µs a run before, 373 after.
