
<details><summary>overview</summary>

- Mesh entities: 36847 drawn, 59103 hidden, 161 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 36288 meshes, 13.9 M raw tris in view, 0.2 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.13 s), plaza (168 ent, 0.01 s)

Zone spawn slices (main thread):

| zone | slice (owner) | main thread, ms |
|---|---|---|
| lake | water demo | 127.8 |
| field | fire demo | 18.3 |
| fortress | destruction demo | 11.0 |
| plaza | showcase maypole | 4.9 |
| plaza | showcase stalls | 2.6 |
| lake | physics floaters | 1.6 |
| plaza | showcase oaks and benches | 0.5 |
| forest | demo | 0.3 |
| field | showcase field edge | 0.2 |
| plaza | showcase paving and fountain | 0.1 |
| fortress | showcase fuel | 0.0 |
| field | showcase smoke air | 0.0 |

- Drawn by feature: meshes 133 draws 5.29 M; terrain 1211 draws 1.65 M; foliage scenery 2 draws 0.27 M; grass 126 draws 0.08 M; water 3 draws 0.05 M; painted ground 1 draws 0.00 M; space bodies 2 draws 0.00 M; fire 1 draws 0.00 M
- Mesh batches: 1653 opaque, 2 transparent, shadow [0,0,0,8,16,20], far shadow null, impostors in view 4027

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / engine_foliage::PlantPart | 27499 | 17507 | 8.29 M | 0.24 M |
| always / showcase::WallHeroes [Lod] | 65 | 65 | 4.84 M | 0.00 M |
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
| always / showcase::WallHeroes [Lod] (mesh 270) | 90880 | 13 | 13 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 252) | 102080 | 10 | 10 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1520) | 1388 | 831 | 620 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1495) | 1364 | 808 | 604 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1570) | 1316 | 815 | 590 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1545) | 1312 | 810 | 577 | 0.02 M | on |
| always / showcase::WallHeroes [Lod] (mesh 264) | 88640 | 8 | 8 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 271) | 92800 | 6 | 6 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 258) | 87040 | 6 | 6 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1124) | 724 | 1293 | 622 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1165) | 772 | 1255 | 579 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1206) | 752 | 1245 | 588 | 0.01 M | on |

- Systems (ms a frame, all threads 15.1 of 18.3): engine_render::render_system 5.99; engine_weather::ground_system 0.63; engine_fire::update_grass 0.39; engine_foliage::sync_scenery_trees 0.35; engine_terrain::update_landmarks 0.32; showcase::run 0.31; engine_audio::vehicle_sounds 0.30; engine_weather::cloud_map_system 0.30

</details>

<details><summary>plaza</summary>

- Mesh entities: 31379 drawn, 54458 hidden, 161 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 30310 meshes, 6.4 M raw tris in view, 4.9 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.13 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 160 draws 5.22 M; terrain 1554 draws 1.66 M; grass 160 draws 0.84 M; foliage scenery 10 draws 0.21 M; water 3 draws 0.06 M; vfx_particles 6 draws 0.00 M; space bodies 1 draws 0.00 M; fire 1 draws 0.00 M
- Mesh batches: 1314 opaque, 1 transparent, shadow [36,123,183,121,162,50], far shadow null, impostors in view 693

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / engine_foliage::PlantPart | 21140 | 7010 | 4.34 M | 2.06 M |
| always / engine_render::ResolvedMesh [Lod] | 3361 | 841 | 1.11 M | 1.99 M |
| always / Kitchen garden [Lod] | 52 | 41 | 0.59 M | 0.72 M |
| lake / Part | 33 | 1 | 0.10 M | 0.35 M |
| always / engine_render::ResolvedMesh | 5269 | 1094 | 0.35 M | 0.01 M |
| plaza / engine_foliage::PlantPart | 12 | 11 | 0.23 M | 0.02 M |
| plaza / engine_render::ResolvedMesh [Lod] | 104 | 98 | 0.17 M | 0.17 M |
| always / showcase::WallHeroes [Lod] | 65 | 0 | 0.00 M | 0.10 M |
| fortress / engine_destruction::PropMesh | 44 | 0 | 0.00 M | 0.07 M |
| plaza / engine_render::ResolvedMesh | 5 | 5 | 0.05 M | 0.05 M |
| fortress / Gatehouse | 4 | 0 | 0.00 M | 0.04 M |
| fortress / North wall | 3 | 0 | 0.00 M | 0.04 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / engine_foliage::PlantPart (mesh 1495) | 1364 | 776 | 182 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1520) | 1388 | 794 | 171 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1570) | 1316 | 788 | 159 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1545) | 1312 | 781 | 155 | 0.01 M | on |
| plaza / engine_foliage::PlantPart (mesh 1159) | 154124 | 1 | 1 | 0.00 M | on |
| always / engine_render::ResolvedMesh [Lod] (mesh 162) | 23196 | 6 | 4 | 0.14 M | on |
| always / engine_render::ResolvedMesh [Lod] (mesh 210) | 16924 | 8 | 4 | 0.14 M | on |
| always / engine_render::ResolvedMesh [Lod] (mesh 194) | 25784 | 5 | 2 | 0.13 M | on |
| always / engine_foliage::PlantPart (mesh 1165) | 772 | 616 | 153 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1083) | 720 | 638 | 164 | 0.01 M | on |
| always / engine_render::ResolvedMesh [Lod] (mesh 146) | 16668 | 7 | 2 | 0.12 M | on |
| always / engine_foliage::PlantPart (mesh 1124) | 724 | 654 | 152 | 0.01 M | on |

- Systems (ms a frame, all threads 207.9 of 209.4): engine_foliage::stream_far_woods 179.30; engine_render::render_system 13.18; showcase::run 3.85; engine_terrain::update_colliders 1.83; engine_foliage::ecology_step 1.62; engine_water::ripple_system 0.73; engine_weather::ground_system 0.52; engine_foliage::sync_plant_visuals 0.39

</details>

<details><summary>forest</summary>

- Mesh entities: 33048 drawn, 54503 hidden, 164 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 32468 meshes, 7.1 M raw tris in view, 6.0 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.13 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 171 draws 6.00 M; terrain 1535 draws 1.61 M; grass 66 draws 0.23 M; foliage scenery 39 draws 0.21 M; water 3 draws 0.06 M; space bodies 2 draws 0.00 M
- Mesh batches: 1096 opaque, 0 transparent, shadow [32,134,159,99,212,74], far shadow null, impostors in view 332

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / engine_foliage::PlantPart | 23245 | 7189 | 5.91 M | 4.35 M |
| always / showcase::WallHeroes [Lod] | 118 | 11 | 0.75 M | 0.18 M |
| always / engine_render::ResolvedMesh [Lod] | 3214 | 450 | 0.08 M | 0.70 M |
| always / Kitchen garden [Lod] | 50 | 0 | 0.00 M | 0.66 M |
| always / engine_render::ResolvedMesh | 5282 | 948 | 0.40 M | 0.00 M |
| lake / Part | 33 | 0 | 0.00 M | 0.15 M |
| fortress / engine_destruction::PropMesh | 44 | 0 | 0.00 M | 0.15 M |
| fortress / Corner tower | 10 | 0 | 0.00 M | 0.06 M |
| plaza / engine_render::ResolvedMesh | 5 | 0 | 0.00 M | 0.05 M |
| fortress / Gatehouse | 4 | 0 | 0.00 M | 0.04 M |
| fortress / North wall | 3 | 0 | 0.00 M | 0.04 M |
| always / engine_foliage::ShadowStandIn | 45 | 0 | 0.00 M | 0.04 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] (mesh 253) | 94400 | 3 | 2 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 259) | 88320 | 7 | 2 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1443) | 2792 | 60 | 60 | 0.17 M | on |
| always / engine_foliage::PlantPart (mesh 1493) | 3260 | 48 | 42 | 0.16 M | on |
| always / engine_foliage::PlantPart (mesh 1568) | 3028 | 48 | 46 | 0.15 M | on |
| always / engine_foliage::PlantPart (mesh 1518) | 3116 | 45 | 44 | 0.14 M | on |
| always / engine_foliage::PlantPart (mesh 1418) | 2232 | 61 | 61 | 0.14 M | on |
| always / engine_foliage::PlantPart (mesh 1468) | 2820 | 47 | 47 | 0.13 M | on |
| always / engine_foliage::PlantPart (mesh 1393) | 2236 | 59 | 59 | 0.13 M | on |
| always / engine_foliage::PlantPart (mesh 1081) | 1720 | 75 | 66 | 0.13 M | on |
| always / engine_foliage::PlantPart (mesh 1495) | 1364 | 750 | 92 | 0.04 M | on |
| always / engine_foliage::PlantPart (mesh 1206) | 752 | 690 | 164 | 0.02 M | on |

- Systems (ms a frame, all threads 49.4 of 50.3): engine_foliage::stream_far_woods 18.48; engine_render::render_system 11.01; engine_water::ripple_system 3.73; showcase::run 1.80; engine_water::boat_air_system 1.60; engine_smoke::step 1.32; engine_fire::step_field 1.05; engine_weather::ground_system 0.74

</details>

<details><summary>lake</summary>

- Mesh entities: 30971 drawn, 55266 hidden, 165 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 30391 meshes, 4.7 M raw tris in view, 2.1 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.13 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 118 draws 2.86 M; grass 7 draws 1.68 M; terrain 1753 draws 1.66 M; foliage scenery 1 draws 0.21 M; water 3 draws 0.12 M; vfx_particles 24 draws 0.00 M; space bodies 4 draws 0.00 M
- Mesh batches: 1004 opaque, 0 transparent, shadow [10,27,83,83,87,55], far shadow null, impostors in view 1447

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / engine_foliage::PlantPart | 21108 | 7876 | 4.00 M | 0.86 M |
| lake / Part | 33 | 33 | 0.87 M | 0.87 M |
| always / engine_render::ResolvedMesh [Lod] | 3273 | 823 | 0.35 M | 0.66 M |
| always / Kitchen garden [Lod] | 46 | 0 | 0.00 M | 0.58 M |
| always / engine_render::ResolvedMesh | 5278 | 1062 | 0.31 M | 0.00 M |
| plaza / engine_render::ResolvedMesh | 5 | 0 | 0.00 M | 0.05 M |
| always / showcase::WallHeroes [Lod] | 36 | 0 | 0.00 M | 0.04 M |
| plaza / engine_render::ResolvedMesh [Lod] | 21 | 0 | 0.00 M | 0.03 M |
| field / Fire demo: shed roof | 1 | 0 | 0.00 M | 0.02 M |
| field / Fire demo: tree 1 trunk | 1 | 0 | 0.00 M | 0.02 M |
| field / Fire demo: tree 2 trunk | 1 | 0 | 0.00 M | 0.02 M |
| field / Fire demo: tree 3 trunk | 1 | 0 | 0.00 M | 0.02 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / engine_foliage::PlantPart (mesh 1520) | 1388 | 810 | 368 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1570) | 1316 | 800 | 372 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1545) | 1312 | 789 | 372 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1495) | 1364 | 795 | 353 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1165) | 772 | 647 | 290 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1083) | 720 | 669 | 307 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1124) | 724 | 693 | 291 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1206) | 752 | 658 | 274 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1445) | 1096 | 262 | 103 | 0.01 M | on |
| lake / Part (mesh 1007) | 111888 | 1 | 1 | 0.11 M | on |
| lake / Part (mesh 1008) | 100848 | 1 | 1 | 0.10 M | on |
| lake / Part (mesh 1004) | 100848 | 1 | 1 | 0.10 M | on |

- Systems (ms a frame, all threads 116.6 of 112.1): engine_water::ripple_system 76.58; engine_render::render_system 15.58; engine_water::shore_wetness_system 5.31; engine_physics::physics_simulate 3.81; engine_water::flow_step_system 2.59; engine_water::boat_air_system 1.52; engine_npc::rebuild_world_model 1.11; engine_weather::ground_system 0.86

</details>

<details><summary>fortress</summary>

- Mesh entities: 35974 drawn, 56363 hidden, 163 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 35396 meshes, 5.4 M raw tris in view, 4.8 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.13 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 109 draws 2.71 M; grass 196 draws 1.72 M; terrain 1519 draws 1.71 M; foliage scenery 3 draws 0.27 M; water 3 draws 0.05 M; vfx_particles 15 draws 0.00 M; sprites 1 draws 0.00 M; space bodies 1 draws 0.00 M; fire 2 draws 0.00 M; smoke 7 draws 0.00 M
- Mesh batches: 1810 opaque, 0 transparent, shadow [0,245,504,295,461,29], far shadow null, impostors in view 1816

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / engine_foliage::PlantPart | 24967 | 5275 | 2.73 M | 4.38 M |
| always / showcase::WallHeroes [Lod] | 174 | 27 | 2.09 M | 0.23 M |
| always / engine_render::ResolvedMesh | 5487 | 1060 | 0.36 M | 0.04 M |
| always / engine_render::ResolvedMesh [Lod] | 3300 | 699 | 0.17 M | 0.06 M |
| fortress / engine_destruction::PropMesh | 44 | 44 | 0.15 M | 0.15 M |
| fortress / Corner tower | 10 | 10 | 0.06 M | 0.06 M |
| fortress / Voxel cliff | 112 | 112 | 0.05 M | 0.05 M |
| fortress / Gatehouse | 4 | 4 | 0.04 M | 0.04 M |
| fortress / North wall | 3 | 3 | 0.04 M | 0.04 M |
| fortress / Stone block | 29 | 29 | 0.04 M | 0.04 M |
| fortress / South wall | 6 | 6 | 0.04 M | 0.04 M |
| plaza / engine_render::ResolvedMesh | 5 | 0 | 0.00 M | 0.04 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] (mesh 270) | 90880 | 26 | 6 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 252) | 102080 | 23 | 5 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1520) | 1388 | 732 | 209 | 0.08 M | on |
| always / engine_foliage::PlantPart (mesh 1518) | 3116 | 91 | 18 | 0.28 M | on |
| always / showcase::WallHeroes [Lod] (mesh 253) | 94400 | 10 | 3 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1495) | 1364 | 728 | 200 | 0.10 M | on |
| always / engine_foliage::PlantPart (mesh 1570) | 1316 | 740 | 207 | 0.07 M | on |
| always / showcase::WallHeroes [Lod] (mesh 264) | 88640 | 33 | 3 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1545) | 1312 | 720 | 201 | 0.07 M | on |
| always / engine_foliage::PlantPart (mesh 1543) | 3004 | 85 | 14 | 0.25 M | on |
| always / engine_foliage::PlantPart (mesh 1493) | 3260 | 75 | 14 | 0.24 M | on |
| always / engine_foliage::PlantPart (mesh 1568) | 3028 | 72 | 9 | 0.22 M | on |

- Systems (ms a frame, all threads 29.8 of 30.2): engine_render::render_system 8.00; engine_physics::physics_simulate 4.05; engine_smoke::step 1.74; engine_destruction::simulate_debris_system 1.30; engine_physics::physics_sync 0.85; engine_fire::step_objects 0.84; engine_weather::ground_system 0.81; engine_npc::rebuild_world_model 0.60

</details>

<details><summary>field</summary>

- Mesh entities: 33192 drawn, 54124 hidden, 163 built-in shapes, 14477 with the showcase's `Lod`. Outside any cell (always loaded): 31416 meshes, 6.4 M raw tris in view, 1.4 M casting.
- Cells loaded: field (1330 ent, 0.02 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.13 s), plaza (168 ent, 0.01 s)
- Drawn by feature: meshes 115 draws 3.68 M; grass 79 draws 2.25 M; terrain 1564 draws 1.73 M; foliage scenery 2 draws 0.21 M; grass 126 draws 0.10 M; water 3 draws 0.06 M; painted ground 1 draws 0.00 M; fire 2 draws 0.00 M; smoke 4 draws 0.00 M
- Mesh batches: 943 opaque, 0 transparent, shadow [2,15,52,73,62,31], far shadow null, impostors in view 1214

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / showcase::WallHeroes [Lod] | 126 | 59 | 4.12 M | 0.21 M |
| always / engine_foliage::PlantPart | 21092 | 3411 | 1.80 M | 0.48 M |
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
| always / showcase::WallHeroes [Lod] (mesh 258) | 87040 | 15 | 10 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 270) | 90880 | 21 | 9 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 259) | 88320 | 7 | 6 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 264) | 88640 | 17 | 5 | 0.00 M | on |
| field / engine_render::ResolvedMesh [Lod] (mesh 129) | 6500 | 79 | 65 | 0.00 M | on |
| field / engine_render::ResolvedMesh [Lod] (mesh 131) | 6500 | 69 | 63 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 252) | 102080 | 12 | 4 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 265) | 96000 | 9 | 4 | 0.00 M | on |
| always / showcase::WallHeroes [Lod] (mesh 253) | 94400 | 10 | 4 | 0.00 M | on |
| field / engine_render::ResolvedMesh [Lod] (mesh 133) | 6500 | 53 | 44 | 0.00 M | on |
| field / engine_render::ResolvedMesh [Lod] (mesh 135) | 6500 | 56 | 40 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 1545) | 1312 | 799 | 158 | 0.01 M | on |

- Systems (ms a frame, all threads 2245.8 of 1260.2): engine_foliage::stream_far_woods 1155.86; engine_render::render_system 304.48; engine_physics::physics_simulate 87.85; engine_water::ripple_system 66.12; engine_smoke::step 51.84; engine_destruction::simulate_debris_system 51.33; showcase::run 48.85; engine_npc::rebuild_world_model 42.72

</details>
