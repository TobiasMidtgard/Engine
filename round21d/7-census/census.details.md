
<details><summary>overview</summary>

- Mesh entities: 37320 drawn, 58630 hidden, 161 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 36761 meshes, 48.5 M raw tris in view, 0.2 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.15 s), plaza (168 ent, 0.01 s)

Zone spawn slices (main thread):

| zone | slice (owner) | main thread, ms |
|---|---|---|
| lake | water demo | 141.8 |
| field | fire demo | 17.2 |
| fortress | destruction demo | 10.7 |
| plaza | showcase maypole | 4.9 |
| plaza | showcase stalls | 2.5 |
| lake | physics floaters | 1.7 |
| plaza | showcase oaks and benches | 0.5 |
| field | showcase field edge | 0.3 |
| forest | demo | 0.2 |
| plaza | showcase paving and fountain | 0.1 |
| field | showcase smoke air | 0.0 |
| fortress | showcase fuel | 0.0 |

- Drawn by feature: meshes 141 draws 5.68 M; terrain 1217 draws 1.66 M; foliage scenery 2 draws 0.27 M; grass 126 draws 0.08 M; water 3 draws 0.05 M; painted ground 1 draws 0.00 M; space bodies 2 draws 0.00 M; fire 1 draws 0.00 M
- Mesh batches: 1652 opaque, 2 transparent, shadow [0,0,0,8,14,0], far shadow null, impostors in view 4144

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] | 538 | 538 | 39.43 M | 0.00 M |
| always / engine_foliage::PlantPart | 27499 | 17507 | 8.29 M | 0.24 M |
| lake / Part | 33 | 33 | 0.87 M | 0.00 M |
| always / engine_render::ResolvedMesh [Lod] | 3043 | 2909 | 0.46 M | 0.00 M |
| always / engine_render::ResolvedMesh | 5254 | 1113 | 0.30 M | 0.00 M |
| fortress / engine_destruction::PropMesh | 44 | 44 | 0.15 M | 0.00 M |
| fortress / Corner tower | 10 | 10 | 0.06 M | 0.00 M |
| plaza / engine_render::ResolvedMesh | 5 | 5 | 0.05 M | 0.00 M |
| fortress / Gatehouse | 4 | 4 | 0.04 M | 0.00 M |
| fortress / North wall | 3 | 3 | 0.04 M | 0.00 M |
| fortress / Stone block | 29 | 29 | 0.04 M | 0.00 M |
| fortress / South wall | 6 | 6 | 0.04 M | 0.00 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] (mesh 270) | 90880 | 81 | 81 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 252) | 102080 | 70 | 70 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 264) | 88640 | 78 | 78 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 258) | 87040 | 71 | 71 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 271) | 92800 | 35 | 35 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 259) | 88320 | 33 | 33 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 253) | 94400 | 28 | 28 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 265) | 96000 | 23 | 23 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1520) | 1388 | 831 | 620 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1495) | 1364 | 808 | 604 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1570) | 1316 | 815 | 590 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1545) | 1312 | 810 | 577 | 0.02 M | on |

- Systems (ms a frame, all threads 15.7 of 19.3): engine_render::render_system 6.53; engine_weather::ground_system 0.69; engine_fire::update_grass 0.41; engine_foliage::sync_scenery_trees 0.35; engine_terrain::update_landmarks 0.33; engine_weather::cloud_map_system 0.33; engine_audio::vehicle_sounds 0.31; showcase::run 0.28

</details>

<details><summary>plaza</summary>

- Mesh entities: 31858 drawn, 53938 hidden, 161 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 30789 meshes, 19.4 M raw tris in view, 5.5 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.15 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 194 draws 6.55 M; terrain 1572 draws 1.69 M; grass 160 draws 0.84 M; foliage scenery 10 draws 0.21 M; water 3 draws 0.06 M; vfx_particles 6 draws 0.00 M; space bodies 1 draws 0.00 M; fire 1 draws 0.00 M
- Mesh batches: 1328 opaque, 1 transparent, shadow [36,123,185,124,163,54], far shadow null, impostors in view 163

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] | 538 | 177 | 13.00 M | 0.66 M |
| always / engine_foliage::PlantPart | 21142 | 7012 | 4.34 M | 2.04 M |
| always / engine_render::ResolvedMesh [Lod] | 3361 | 841 | 1.11 M | 1.99 M |
| always / Kitchen garden [Lod] | 52 | 41 | 0.59 M | 0.72 M |
| lake / Part | 33 | 1 | 0.10 M | 0.35 M |
| always / engine_render::ResolvedMesh | 5269 | 1094 | 0.35 M | 0.01 M |
| plaza / engine_foliage::PlantPart | 12 | 11 | 0.23 M | 0.02 M |
| plaza / engine_render::ResolvedMesh [Lod] | 104 | 98 | 0.17 M | 0.17 M |
| fortress / engine_destruction::PropMesh | 44 | 0 | 0.00 M | 0.07 M |
| plaza / engine_render::ResolvedMesh | 5 | 5 | 0.05 M | 0.05 M |
| fortress / Gatehouse | 4 | 0 | 0.00 M | 0.04 M |
| fortress / North wall | 3 | 0 | 0.00 M | 0.04 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] (mesh 270) | 90880 | 81 | 33 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 252) | 102080 | 70 | 24 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 258) | 87040 | 71 | 23 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 264) | 88640 | 78 | 19 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 259) | 88320 | 33 | 14 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 265) | 96000 | 23 | 9 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 271) | 92800 | 35 | 9 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 253) | 94400 | 28 | 7 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1495) | 1364 | 776 | 182 | 0.01 M | on |
| always / showcase::WallHeroes [Lod] (mesh 277) | 6720 | 43 | 14 | 0.24 M | on |
| always / engine_foliage::PlantPart (mesh 1520) | 1388 | 794 | 171 | 0.01 M | on |
| always / showcase::WallHeroes [Lod] (mesh 278) | 7040 | 37 | 12 | 0.21 M | on |

- Systems (ms a frame, all threads 76.0 of 78.5): engine_foliage::stream_far_woods 54.78; engine_render::render_system 8.85; engine_water::ripple_system 2.12; showcase::run 1.43; engine_weather::ground_system 0.60; engine_terrain::update_colliders 0.60; engine_foliage::ecology_step 0.47; engine_fire::update_grass 0.36

</details>

<details><summary>forest</summary>

- Mesh entities: 33502 drawn, 53996 hidden, 165 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 32922 meshes, 19.5 M raw tris in view, 6.4 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.15 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 225 draws 6.43 M; terrain 1532 draws 1.60 M; grass 66 draws 0.23 M; foliage scenery 39 draws 0.21 M; water 3 draws 0.06 M; space bodies 2 draws 0.00 M
- Mesh batches: 1105 opaque, 0 transparent, shadow [32,133,159,104,211,74], far shadow null, impostors in view 412

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] | 576 | 176 | 13.06 M | 0.66 M |
| always / engine_foliage::PlantPart | 23248 | 7192 | 5.93 M | 4.35 M |
| always / engine_render::ResolvedMesh [Lod] | 3214 | 450 | 0.08 M | 0.70 M |
| always / Kitchen garden [Lod] | 50 | 0 | 0.00 M | 0.66 M |
| always / engine_render::ResolvedMesh | 5282 | 948 | 0.40 M | 0.00 M |
| lake / Part | 33 | 0 | 0.00 M | 0.15 M |
| fortress / engine_destruction::PropMesh | 44 | 0 | 0.00 M | 0.15 M |
| fortress / Corner tower | 10 | 0 | 0.00 M | 0.06 M |
| plaza / engine_render::ResolvedMesh | 5 | 0 | 0.00 M | 0.05 M |
| always / engine_foliage::ShadowStandIn | 47 | 0 | 0.00 M | 0.05 M |
| fortress / Gatehouse | 4 | 0 | 0.00 M | 0.04 M |
| fortress / North wall | 3 | 0 | 0.00 M | 0.04 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] (mesh 264) | 88640 | 82 | 30 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 258) | 87040 | 85 | 27 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 252) | 102080 | 83 | 23 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 270) | 90880 | 88 | 20 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 271) | 92800 | 37 | 13 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 265) | 96000 | 29 | 10 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 253) | 94400 | 25 | 8 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 259) | 88320 | 28 | 8 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 276) | 6720 | 45 | 15 | 0.24 M | on |
| always / showcase::WallHeroes [Lod] (mesh 278) | 7040 | 36 | 12 | 0.23 M | on |
| always / showcase::WallHeroes [Lod] (mesh 277) | 6720 | 38 | 10 | 0.19 M | on |
| always / engine_foliage::PlantPart (mesh 1443) | 2792 | 60 | 60 | 0.17 M | on |

- Systems (ms a frame, all threads 54.9 of 56.1): engine_foliage::stream_far_woods 20.75; engine_render::render_system 10.32; engine_water::ripple_system 8.36; showcase::run 1.94; engine_water::boat_air_system 1.49; engine_smoke::step 1.32; engine_weather::ground_system 0.78; engine_audio::water_ambience 0.55

</details>

<details><summary>lake</summary>

- Mesh entities: 31508 drawn, 54684 hidden, 164 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 30928 meshes, 10.0 M raw tris in view, 2.8 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.15 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 138 draws 3.41 M; grass 7 draws 1.68 M; terrain 1753 draws 1.66 M; foliage scenery 1 draws 0.21 M; water 3 draws 0.12 M; vfx_particles 18 draws 0.00 M; space bodies 4 draws 0.00 M
- Mesh batches: 1011 opaque, 0 transparent, shadow [10,27,83,83,85,55], far shadow null, impostors in view 1625

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] | 573 | 72 | 5.38 M | 0.68 M |
| always / engine_foliage::PlantPart | 21109 | 7876 | 4.00 M | 0.86 M |
| lake / Part | 33 | 33 | 0.87 M | 0.87 M |
| always / engine_render::ResolvedMesh [Lod] | 3273 | 823 | 0.35 M | 0.66 M |
| always / Kitchen garden [Lod] | 46 | 0 | 0.00 M | 0.58 M |
| always / engine_render::ResolvedMesh | 5278 | 1062 | 0.31 M | 0.00 M |
| plaza / engine_render::ResolvedMesh | 5 | 0 | 0.00 M | 0.05 M |
| plaza / engine_render::ResolvedMesh [Lod] | 21 | 0 | 0.00 M | 0.03 M |
| field / Fire demo: shed roof | 1 | 0 | 0.00 M | 0.02 M |
| field / Fire demo: tree 1 trunk | 1 | 0 | 0.00 M | 0.02 M |
| field / Fire demo: tree 2 trunk | 1 | 0 | 0.00 M | 0.02 M |
| field / Fire demo: tree 3 trunk | 1 | 0 | 0.00 M | 0.02 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] (mesh 258) | 87040 | 88 | 20 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 270) | 90880 | 88 | 11 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 252) | 102080 | 74 | 7 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 264) | 88640 | 80 | 7 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1520) | 1388 | 810 | 368 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1570) | 1316 | 800 | 372 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1545) | 1312 | 789 | 372 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1495) | 1364 | 795 | 353 | 0.02 M | on |
| always / showcase::WallHeroes [Lod] (mesh 253) | 94400 | 29 | 4 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 259) | 88320 | 29 | 4 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 265) | 96000 | 29 | 3 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 277) | 6720 | 40 | 5 | 0.23 M | on |

- Systems (ms a frame, all threads 115.4 of 111.1): engine_water::ripple_system 76.07; engine_render::render_system 14.58; engine_water::shore_wetness_system 5.50; engine_physics::physics_simulate 4.01; engine_water::flow_step_system 2.45; engine_water::boat_air_system 1.59; engine_npc::rebuild_world_model 1.33; engine_weather::ground_system 0.85

</details>

<details><summary>fortress</summary>

- Mesh entities: 36562 drawn, 55832 hidden, 163 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 35991 meshes, 15.1 M raw tris in view, 5.4 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.15 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 129 draws 3.93 M; grass 196 draws 1.72 M; terrain 1519 draws 1.71 M; foliage scenery 3 draws 0.27 M; water 3 draws 0.05 M; vfx_particles 26 draws 0.00 M; sprites 1 draws 0.00 M; space bodies 1 draws 0.00 M; fire 2 draws 0.00 M; smoke 7 draws 0.00 M
- Mesh batches: 1812 opaque, 0 transparent, shadow [0,270,505,293,466,29], far shadow null, impostors in view 1888

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] | 616 | 156 | 11.79 M | 0.76 M |
| always / engine_foliage::PlantPart | 24971 | 5275 | 2.73 M | 4.37 M |
| always / engine_render::ResolvedMesh | 5487 | 1060 | 0.36 M | 0.04 M |
| always / engine_render::ResolvedMesh [Lod] | 3300 | 699 | 0.17 M | 0.06 M |
| fortress / engine_destruction::PropMesh | 44 | 44 | 0.15 M | 0.15 M |
| fortress / Corner tower | 10 | 10 | 0.06 M | 0.06 M |
| always / engine_foliage::ShadowStandIn | 52 | 0 | 0.00 M | 0.05 M |
| fortress / Gatehouse | 4 | 4 | 0.04 M | 0.04 M |
| fortress / North wall | 3 | 3 | 0.04 M | 0.04 M |
| fortress / Voxel cliff | 105 | 105 | 0.04 M | 0.04 M |
| fortress / Stone block | 29 | 29 | 0.04 M | 0.04 M |
| fortress / South wall | 6 | 6 | 0.04 M | 0.04 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] (mesh 270) | 90880 | 99 | 37 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 252) | 102080 | 93 | 23 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 258) | 87040 | 82 | 19 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 264) | 88640 | 96 | 15 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 253) | 94400 | 27 | 9 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 271) | 92800 | 39 | 8 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 265) | 96000 | 30 | 7 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 259) | 88320 | 27 | 7 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1520) | 1388 | 732 | 209 | 0.08 M | on |
| always / engine_foliage::PlantPart (mesh 1518) | 3116 | 91 | 18 | 0.28 M | on |
| always / engine_foliage::PlantPart (mesh 1495) | 1364 | 728 | 200 | 0.10 M | on |
| always / engine_foliage::PlantPart (mesh 1570) | 1316 | 740 | 207 | 0.07 M | on |

- Systems (ms a frame, all threads 31.1 of 31.7): engine_render::render_system 7.39; engine_physics::physics_simulate 5.48; engine_smoke::step 1.74; engine_physics::physics_sync 1.71; engine_destruction::simulate_debris_system 1.26; engine_npc::rebuild_world_model 1.06; engine_fire::step_objects 0.89; engine_weather::ground_system 0.84

</details>

<details><summary>field</summary>

- Mesh entities: 33760 drawn, 53553 hidden, 163 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 31991 meshes, 16.5 M raw tris in view, 1.8 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.15 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 132 draws 4.03 M; grass 79 draws 2.25 M; terrain 1564 draws 1.73 M; foliage scenery 2 draws 0.21 M; grass 126 draws 0.10 M; water 3 draws 0.06 M; painted ground 1 draws 0.00 M; fire 2 draws 0.00 M; smoke 4 draws 0.00 M
- Mesh batches: 953 opaque, 0 transparent, shadow [2,14,51,72,61,31], far shadow null, impostors in view 1276

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] | 607 | 187 | 14.30 M | 0.64 M |
| always / engine_foliage::PlantPart | 21097 | 3411 | 1.80 M | 0.48 M |
| field / engine_render::ResolvedMesh [Lod] | 1202 | 1113 | 1.76 M | 0.00 M |
| always / engine_render::ResolvedMesh [Lod] | 3218 | 418 | 0.07 M | 0.44 M |
| always / engine_render::ResolvedMesh | 5473 | 991 | 0.38 M | 0.01 M |
| always / Kitchen garden [Lod] | 34 | 0 | 0.00 M | 0.23 M |
| lake / Part | 33 | 0 | 0.00 M | 0.15 M |
| plaza / engine_render::ResolvedMesh | 5 | 0 | 0.00 M | 0.05 M |
| plaza / engine_render::ResolvedMesh [Lod] | 21 | 0 | 0.00 M | 0.03 M |
| field / Fire demo: shed roof | 1 | 1 | 0.02 M | 0.02 M |
| field / Fire demo: tree 1 trunk | 1 | 1 | 0.02 M | 0.02 M |
| field / Fire demo: tree 2 trunk | 1 | 1 | 0.02 M | 0.02 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] (mesh 264) | 88640 | 99 | 37 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 252) | 102080 | 86 | 25 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 270) | 90880 | 95 | 28 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 258) | 87040 | 84 | 27 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 265) | 96000 | 25 | 10 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 259) | 88320 | 31 | 10 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 253) | 94400 | 31 | 8 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 271) | 92800 | 34 | 8 | 0.00 M | on |
| field / engine_render::ResolvedMesh [Lod] (mesh 129) | 6500 | 79 | 65 | 0.00 M | on |
| field / engine_render::ResolvedMesh [Lod] (mesh 131) | 6500 | 69 | 63 | 0.00 M | on |
| field / engine_render::ResolvedMesh [Lod] (mesh 133) | 6500 | 53 | 44 | 0.00 M | on |
| field / engine_render::ResolvedMesh [Lod] (mesh 135) | 6500 | 56 | 40 | 0.00 M | on |

- Systems (ms a frame, all threads 272.2 of 161.4): engine_foliage::stream_far_woods 124.68; engine_render::render_system 38.51; engine_physics::physics_simulate 16.95; engine_water::ripple_system 7.18; engine_smoke::step 6.95; engine_destruction::simulate_debris_system 6.95; engine_npc::rebuild_world_model 5.84; showcase::run 5.69

</details>
