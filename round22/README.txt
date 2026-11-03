Round 22 on Tobias's PC (RTX 5080, 32 threads, 64 GB), main 5de4f33e (after fd471748), release, Ultra, window 1600x900
as the exe opens it, real time, every showcase run with --profile. Every game run was started only after 5+ min with no
input, one at a time (no build beside a render), and ended with taskkill /IM showcase.exe /F. Input only through the
game's flags and bridge.
1-route-full, 1-route-up   route and route up: leg tables, hitches, RAM per leg, a picture per leg; soft bodies, water
                           ripples, terrain colliders and the worst frames' terrain rows on the 2 km legs / 79 m-1 km
2a-loops-ultra             loops x2 at Ultra with PHYSICS_REPORT=240: tables, per-pass GPU, physics reports
2b-render-scale-0.7        the same loops at ENGINE_RENDER_SCALE=0.7: per-pass GPU 1.0 vs 0.7; round 2 physics (CCD sweeps)
3-mesh-occlusion           ENGINE_MESH_OCCLUSION on/off x2 at forest_trail, plaza, market, village_garden (for 'town street')
4-lake-forest-stays        lake, forest_trail and plaza stays: scene/prepass rows, extra views, probe GI, mesh uploads
