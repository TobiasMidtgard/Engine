Round 21c step 7: memory probe at the plaza, main 145dd3a6, Tobias's PC (64 GB), release, Ultra, 1600x900.
showcase.exe --fly --profile --quality ultra; showcase.goto plaza through the bridge, then held 10 min. Every 30 s:
process working set and private bytes (Get-Process), system free RAM, entity count (engine.stats) and terrain.patches_*.
Guard: stop if private > 30 GB or free < 10 GB (never tripped).

s at plaza | working set MB | private MB | free MB | entities | patches building | missing | resident | drawn
0 | 4334 | 7258 | 33618 | 65685 | 75 | 75 | 166 | 49
31 | 5029 | 15235 | 33177 | 120695 | 0 | 0 | 1669 | 751
60 | 4984 | 15445 | 33170 | 123052 | 0 | 0 | 1669 | 751
120 | 4996 | 15458 | 33153 | 124379 | 0 | 0 | 1669 | 751
180 | 4983 | 15445 | 33198 | 124939 | 0 | 0 | 1669 | 751
240 | 4986 | 15447 | 33126 | 125181 | 0 | 0 | 1669 | 751
270 | 4987 | 15447 | 33181 | 125090 | 32 | 0 | 1669 | 751
300 | 4990 | 15453 | 33134 | 125182 | 0 | 0 | 1669 | 751
360 | 4995 | 15457 | 33141 | 124588 | 0 | 0 | 1669 | 751
420 | 4997 | 15458 | 33132 | 124273 | 0 | 0 | 1669 | 751
480 | 4998 | 15458 | 33156 | 124396 | 0 | 0 | 1669 | 751
540 | 5000 | 15459 | 33161 | 124227 | 0 | 0 | 1669 | 751
600 | 5000 | 15459 | 33154 | 124034 | 0 | 0 | 1669 | 751
(every 30 s row in mem.csv)

Reading: it no longer grows. Private bytes 15,235 MB at 31 s and 15,459 MB at 10 min: +210 MB in the first minute,
then +14 MB over the next 9 (21b: ~0.9 GB a minute). Working set flat at ~5.0 GB, system free flat at ~33.1 GB.
Terrain patches all finished within 31 s of arriving (1669 resident, 751 drawn, 0 building, 0 missing) and stayed
finished; one sample at 270 s had 32 building, done by the next. Entities rose 120.7k -> 125.2k over the first 5 min,
then eased to 124.0k. Mesh assets 8839 (1374 MB) and images 1110 (386 MB) at the end (showcase's memory line).

Frame rate over the hold, per minute: 18.9 (incl. arrival), 20.2, 19.8, 20.0, 20.4, 20.7, 20.6, 20.2, 20.1, 20.7 fps.
Steady, but CPU-bound and below step 5's plaza (27.6 fps): physics_simulate ran alone 7.9 ms a frame for the whole
10 min (physics budget back at level 0 from 41 s), water::flow_bed_system 3.4 ms, render_system 12 ms; one system at a
time 39 ms against GPU 25.2 ms. Same pattern as step 6: across runs the plaza's physics_simulate is either ~1-2.5 ms
or ~8-12 ms a frame, and when it is high it stays high.
