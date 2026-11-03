Round 21d on Tobias's PC (RTX 5080, 32 threads, 64 GB), main 1810b539, release, Ultra, window 1600x900, real time.
Every game run was started only after 5+ min with no input, one at a time (no build beside a render), and ended with
taskkill /IM showcase.exe /F. Input only through the game's bridge and flags.
1-route-full, 1-route-up   P6 route: leg tables, hitches, RAM per leg, a picture at each leg
2a-loops-ultra             loops x2 at Ultra, 21c's tables plus the per-pass GPU table
2b-aerial-table            ENGINE_AERIAL_TABLE on/off at overview and forest_overview (woods_aerial stand-in)
3-fixed-step-chain         --tour plaza as asked, and three 75 s plaza stands with ENGINE_PROFILE=1 PHYSICS_REPORT=600
4-render-asks              probe GI / gi.* / meshes.* at forest_trail, plaza, lake; mirror shares lighting on/off
5-terrain-gpu              terrain scene pass: default, TERRAIN_DEPTH=greater, TERRAIN_SHADER_OFF=16 and 256
6-physics-bench            yard_bench -- 20: default (budget off) and PHYSICS_BUDGET_MS=8
7-census                   census tour of six stops
