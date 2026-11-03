Round 22 step 5: SHOWCASE_CENSUS=census.md showcase.exe --tour overview,plaza,forest,lake,fortress,field --profile
--quality ultra on main 5de4f33e, Tobias's PC (RTX 5080), release, window 1600x900. The tour ran all six stops, 37 s from launch,
and exited by itself, code 0; every stop's wait ended "0 not ready" (waited 7, 2, -, 2, 6, 2 s for meshes' clusters,
terrain patches and foliage; the forest stop logged no wait). As in 21d, --tour loads only the six stops' zones
(5 cells), not the full world.

census.md (as written), with 21d's (main 1810b539) in brackets:
| Stop | Entities | Cells | Mesh entities (in view) | Raw tris in view | Raw tris casting | Drawn tris | Draws | Instances | Plants (growing) | Not ready |
| overview | 126159 | 5 | 36847 (22531) | 16.1 M (50.7) | 0.2 M (0.2) | 7.3 M (7.7) | 1482 (1496) | 39933 | 9611 (3388) | 0 |
| plaza    | 112684 | 5 | 31379 (9122)  |  7.0 M (20.0) | 6.4 M (7.0) | 8.0 M (9.3) | 1895 (1947) | 743018 | 9644 (3353) | 0 |
| forest   | 115347 | 5 | 33048 (8620)  |  7.2 M (19.6) | 7.3 M (7.8) | 8.1 M (8.5) | 1816 (1867) | 187393 | 9644 (3323) | 0 |
| lake     | 113203 | 5 | 30971 (9805)  |  5.5 M (10.9) | 3.3 M (4.0) | 6.5 M (7.1) | 1919 (1924) | 204933 | 9644 (3323) | 0 |
| fortress | 121154 | 5 | 35974 (8910)  |  6.4 M (16.1) | 6.0 M (6.5) | 6.5 M (7.7) | 1930 (1981) | 1087175 | 9581 (3347) | 0 |
| field    | 114442 | 5 | 33192 (6013)  |  8.4 M (18.5) | 1.9 M (2.3) | 8.0 M (8.4) | 1896 (1913) | 401080 | 9581 (3242) | 0 |
Raw triangles in view fell 2-3x at every stop (the wall stones drawn to 180 m, c7a510cc); drawn triangles 0.4-1.3 M
lower, draws about the same.

From census.details.md:
- Heaviest group per stop (raw tris in view / casting): overview PlantPart 8.29 M / 0.24 (then WallHeroes 4.84 M, 65
  meshes; 21d: WallHeroes 39.43 M); plaza PlantPart 4.34 / 2.06, ResolvedMesh [Lod] 1.11 / 1.99; forest PlantPart
  5.91 / 4.35; lake PlantPart 4.00 / 0.86, lake Part 0.87 / 0.87; fortress PlantPart 2.73 / 4.38, WallHeroes 2.09 / 0.23;
  field WallHeroes 4.12 M (126 meshes, 59 in view), PlantPart 1.80. So the foliage's plant parts are now the heaviest
  group everywhere but the field.
- Zone spawn on the main thread (overview): lake water demo 127.8 ms (21d 141.8), field fire demo 18.3 (17.2),
  fortress destruction demo 11.0 (10.7), plaza maypole 4.9.
- The census's one-frame "Systems" line catches hitches at three stops: plaza 209.4 ms (foliage::stream_far_woods
  179.3), lake 112.1 ms (water::ripple_system 76.6), field 1260.2 ms (stream_far_woods 1155.9, render_system 304.5,
  physics_simulate 87.9); overview 18.3, forest 50.3 (stream_far_woods 18.5), fortress 30.2.

Files: census.md, census.details.md, jpg/ (each stop), frames.csv, timeline.txt, nvidia_dmon.txt, logs.zip (stderr,
engine-profile.txt).
