Round 21c step 2: cargo run --release -p engine_physics --example yard_bench -- 20 on main 145dd3a6,
headless, nothing else running (no game, no build). Tobias's PC: RTX 5080, 32 threads, 64 GB.
Each setting run twice (r1, r2) to show run-to-run spread. Round 21b numbers (main aac391d4) for comparison.

run | physics step ms: mean | median | p95 | p99 | max | frame ms p95 / max | slowest step line
default (pool 8) r1 | 4.32 | 3.09 | 9.70 | 12.45 | 23.04 | 10.13 / 23.50 | frame 15, 23.04 ms, 1416 awake, 3 regions, 2950 touching pairs, 21 fine, 1 substeps
default (pool 8) r2 | 4.30 | 3.07 | 9.92 | 12.37 | 22.06 | 10.23 / 22.56 | frame 15, 22.06 ms, 1416 awake, 3 regions, 2950 touching pairs, 21 fine, 1 substeps
PHYSICS_WIDE_THREADS=16 r1 | 4.54 | 3.22 | 10.53 | 14.10 | 24.61 | 10.98 / 25.08 | frame 15, 24.61 ms, 1416 awake, 3 regions, 2950 touching pairs, 21 fine, 1 substeps
PHYSICS_WIDE_THREADS=16 r2 | 4.62 | 3.30 | 10.12 | 13.33 | 24.79 | 10.62 / 25.27 | frame 15, 24.79 ms, 1416 awake, 3 regions, 2950 touching pairs, 21 fine, 1 substeps
21b default (aac391d4) | 4.08 | 3.14 | 8.33 | 10.88 | 47.59 | 8.57 / 48.04 | (no slowest-step line then; busiest step 47.59 ms)
21b wide16 (aac391d4) | 4.03 | 3.00 | 8.57 | 11.44 | 48.18 | 8.85 / 48.45 |

Busiest step (all runs): 1492 awake of 1492 bodies, 3 regions, 2931 touching pairs, 7.10 / 7.19 ms (default),
8.89 / 8.49 ms (wide16). Load level 0 at the end in every run. 1200 frames in 5.6-6.0 s.

Reading: the max halved (47-48 ms in 21b -> 22-25 ms now), and the slowest step is frame 15 (the pile's first
big contact burst) every time. Mean, p95 and p99 are 5-20% higher than 21b's on this run. 16 wide threads is
a little slower than the default pool of 8 on every number, in both repeats.
