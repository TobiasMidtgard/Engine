Round 21d step 7: SHOWCASE_CENSUS=census.md showcase.exe --tour overview,plaza,forest,lake,fortress,field --profile
--quality ultra on main 1810b539, Tobias's PC (RTX 5080), release, window as the exe opens it (1600x900). Ultra as
21b's census. The tour ran all six stops in 38 s and exited by itself, code 0; every stop's wait ended with
"0 not ready" (waited 7, 2, 1, 2, 6, 1 s for meshes' clusters, terrain patches and foliage). --tour loads only the
six stops' zones (5 cells), so this is not the full world of steps 1-5.

census.md (as written):
| Stop | Entities | Cells loaded | Mesh entities (in view) | Raw tris in view | Raw tris casting | Drawn tris | Draws | Instances | Plants (growing) | Not ready |
| overview | 126159 | 5 | 37320 (23004) | 50.7 M | 0.2 M | 7.7 M | 1496 | 40412 | 9611 (3388) | 0 |
| plaza | 112624 | 5 | 31858 (9303) | 20.0 M | 7.0 M | 9.3 M | 1947 | 743367 | 9629 (3327) | 0 |
| forest | 115280 | 5 | 33502 (8788) | 19.6 M | 7.8 M | 8.5 M | 1867 | 187881 | 9629 (3302) | 0 |
| lake | 113142 | 5 | 31508 (9877) | 10.9 M | 4.0 M | 7.1 M | 1924 | 205413 | 9629 (3297) | 0 |
| fortress | 121324 | 5 | 36562 (9171) | 16.1 M | 6.5 M | 7.7 M | 1981 | 1088413 | 9554 (3285) | 0 |
| field | 114502 | 5 | 33760 (6141) | 18.5 M | 2.3 M | 8.4 M | 1913 | 401478 | 9554 (3189) | 0 |

From census.details.md: the heaviest group everywhere is "always / showcase::WallHeroes [Lod]": 538-616 meshes,
39.43 M raw tris in view at overview (of 50.7 M), 11.8-14.3 M at the other stops, its meshes 87-102 k triangles each
at full detail. At overview the lake's water demo took 141.8 ms of the main thread to spawn (fire demo 17.2,
destruction demo 10.7). 21b's census (aac391d4, 14 cells, a 16-stop tour) had overview at 13.7 M raw tris in view,
6.6 M drawn, 1755 draws.

Files: census.md, census.details.md, jpg/ (each stop), frames.csv, timeline.txt, nvidia_dmon.txt, logs.zip (stderr,
engine-profile.txt).
