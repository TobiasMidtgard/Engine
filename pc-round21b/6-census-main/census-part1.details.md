
<details><summary>overview</summary>

- Mesh entities: 37883 drawn, 59244 hidden, 1297 built-in shapes, 14524 with the showcase's `Lod`. Outside any cell (always loaded): 36221 meshes, 9.0 M raw tris in view, 0.3 M casting.
- Cells loaded: airfield (183 ent, 0.22 s), field (1330 ent, 0.01 s), forest (464 ent, 0.00 s), fortress (204 ent, 0.01 s), lake (203 ent, 0.11 s), market (138 ent, 0.98 s), meadow (43 ent, 0.00 s), paddock (325 ent, 3.85 s), plaza (168 ent, 0.69 s), spaceport (244 ent, 0.14 s), weather (159 ent, 0.21 s), yard (576 ent, 0.57 s), yard_rinse (3 ent, 0.00 s), yard_sailboat (28 ent, 0.04 s)
- Drawn by feature: meshes 264 draws 3.32 M; terrain 1228 draws 1.61 M; skinned meshes 53 draws 0.93 M; foliage scenery 2 draws 0.27 M; softbody 62 draws 0.26 M; character 6 draws 0.13 M; grass 126 draws 0.08 M; water 3 draws 0.05 M; painted ground 1 draws 0.00 M; space bodies 2 draws 0.00 M
- Mesh batches: 2042 opaque, 22 transparent, shadow [0,0,0,0,57], far shadow 103, impostors in view 6252

| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |
|---|---|---|---|---|
| always / engine_foliage::PlantPart | 27517 | 17515 | 8.25 M | 0.32 M |
| lake / Part | 33 | 33 | 0.87 M | 0.00 M |
| meadow / engine_npc::StandInPart | 790 | 790 | 0.82 M | 0.00 M |
| market / engine_render::ResolvedMesh | 100 | 100 | 0.78 M | 0.00 M |
| always / engine_render::ResolvedMesh [Lod] | 3043 | 2909 | 0.46 M | 0.00 M |
| always / engine_render::ResolvedMesh | 5224 | 1088 | 0.30 M | 0.00 M |
| paddock / engine_anim::IkSurface | 5 | 5 | 0.18 M | 0.00 M |
| yard / Marble statue body | 1 | 1 | 0.16 M | 0.00 M |
| fortress / engine_destruction::PropMesh | 44 | 44 | 0.15 M | 0.00 M |
| spaceport / engine_render::ResolvedMesh [Lod] | 33 | 33 | 0.10 M | 0.00 M |
| weather / Weather station | 20 | 20 | 0.10 M | 0.00 M |
| fortress / Corner tower | 10 | 10 | 0.06 M | 0.00 M |

| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |
|---|---|---|---|---|---|
| always / engine_foliage::PlantPart (mesh 2290) | 1388 | 827 | 623 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 2265) | 1364 | 807 | 604 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 2340) | 1316 | 812 | 592 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 2315) | 1312 | 809 | 578 | 0.02 M | on |
| always / engine_foliage::PlantPart (mesh 1894) | 724 | 1294 | 624 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1935) | 772 | 1250 | 582 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1976) | 752 | 1242 | 589 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 1853) | 720 | 1284 | 594 | 0.01 M | on |
| always / engine_foliage::PlantPart (mesh 2240) | 1224 | 245 | 199 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 2405) | 248 | 979 | 957 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 2215) | 1096 | 269 | 209 | 0.00 M | on |
| always / engine_foliage::PlantPart (mesh 2365) | 224 | 1012 | 995 | 0.00 M | on |

- Systems (ms a frame, all threads 290.7 of 293.4): showcase::run 250.58; engine_render::render_system 24.37; engine_water::boat_air_system 0.97; engine_weather::ground_system 0.88; engine_foliage::sync_scenery_trees 0.72; engine_core::propagate_transforms_system 0.70; engine_physics::physics_simulate 0.60; engine_softbody::step_beside_physics 0.60

</details>
