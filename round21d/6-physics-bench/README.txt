Round 21d step 6: cargo run --release -p engine_physics --example yard_bench -- 20 on main 1810b539, headless, nothing
else running (no game, no build beside it). Tobias's PC: RTX 5080, 32 threads, 64 GB. 1200 frames each.

run | physics step ms: mean | median | p95 | p99 | max | frame ms p95 / max | slowest step | busiest step
default r1 | 7.43 | 7.04 | 12.94 | 15.68 | 19.46 | 13.49 / 19.89 | frame 15, 19.46 ms, 1038 awake, 2 regions, 957 touching pairs, 21 fine, 1 substeps | 1492 awake of 1492, 2 regions, 2931 touching, 9.89 ms
default r2 | 7.31 | 7.03 | 12.47 | 14.73 | 22.43 | 13.07 / 22.92 | frame 278, 22.43 ms, 1276 awake, 2 regions, 5392 touching pairs, 16 fine, 1 substeps | 1492 awake of 1492, 2 regions, 2931 touching, 10.68 ms
PHYSICS_BUDGET_MS=8 r1 | 4.06 | 3.05 | 8.79 | 10.83 | 19.40 | 9.05 / 19.88 | frame 11, 19.40 ms, 1397 awake, 2 regions, 3741 touching pairs, 17 fine, 1 substeps | 1492 awake of 1492, 3 regions, 2917 touching, 8.40 ms
PHYSICS_BUDGET_MS=8 r2 | 4.08 | 3.04 | 8.72 | 11.06 | 19.73 | 8.96 / 20.15 | frame 11, 19.73 ms, 1397 awake, 2 regions, 3741 touching pairs, 17 fine, 1 substeps | 1492 awake of 1492, 3 regions, 2917 touching, 9.55 ms
21c default (145dd3a6, budget 8 ms then) r1 | 4.32 | 3.09 | 9.70 | 12.45 | 23.04 | 10.13 / 23.50 | frame 15, 23.04 ms, 1416 awake, 3 regions, 2950 touching pairs, 21 fine, 1 substeps |
21c default r2 | 4.30 | 3.07 | 9.92 | 12.37 | 22.06 | 10.23 / 22.56 | frame 15, 22.06 ms, ... |
All runs end at load level 0.

Reading: today's default (step budget off since 21a5efc7, "Physics: the step budget reads physics's own work, and is off
by default") steps the yard at 7.4 ms mean / 7.0 median, against 4.1 / 3.05 with the budget on at 8 ms on the same
build, and 4.3 / 3.1 in 21c (the budget was on at 8 ms by default then). step_ms now counts only physics's own work
(narrower than in 21c), so the rise is the work itself, not the measurement. The budget commit's "the yard's busiest step
is near 4 ms on that PC" does not hold with the budget off: the busiest step (all 1492 bodies awake) is 9.9-10.7 ms,
and half the steps are over 7 ms. With the budget at 8 ms the yard is ~0.2-0.9 ms better than 21c everywhere except
the slowest single step (19.4-19.7 vs 22-23 ms, frame 11 instead of 15).
The slowest step is early (frames 11-15, the stations starting) in every run but default r2 (frame 278, 5392 touching pairs).

Files: default_r1/r2.txt, budget8_r1/r2.txt (the bench's own lines), *.err (cargo's build and run lines).
