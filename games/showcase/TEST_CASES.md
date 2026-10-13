# Showcase test cases

Every test case Tobias asked the engine to show, where it lives in the showcase, which thread
owns it, and whether you can see it yet. Viewpoint keys work with `--tour <key>`, the
`showcase.goto` API and the screenshot tour.

The Beautiful showcase world thread keeps this list current. Module threads: your zone is one
line in `zones::spawn_demos` (swap the `ph::…` placeholder for your demo) and one line in
`zones::demo_views` (extra tour stops, with warm-up seconds and API calls on arrive/leave). When
your demo replaces a placeholder, update your rows (status and notes) in the same PR.

**Status:** ✅ live (the module's demo runs there) · 🟨 placeholder (the zone and stand-in props
are built, nothing simulates yet) · ⬜ not visible yet

## Scale and travel

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 1 | Huge scale: walk, drive, fly from the ground to orbit and the Moon without loading screens | `overview`, `tower`, `stratosphere`, `orbit`, `moon` | Planets and terrain | 🟨 camera flies there; planet surface from orbit is flat color |
| 2 | Efficient enough for multiplanetary travel (planets, moons, orbits, reference frames) | `spaceport`, `orbit`, `moon` | Planets and terrain | ⬜ |
| 3 | Cars with real suspension, tires and drivetrain | `track` | Cars, planes and spaceships | 🟨 demo in; being checked on the Rapier backend |
| 4 | Planes with aerodynamic surfaces | `airfield` (1.2 km runway) | Cars, planes and spaceships | 🟨 demo in; being checked on the Rapier backend |
| 5 | Spaceships with thrusters and RCS, launching to orbit | `spaceport` | Cars, planes and spaceships | 🟨 demo in; being checked on the Rapier backend |
| 6 | Extremely well built, efficient physics: stacking, joints, ragdolls | `physics`, `physics_ragdolls`, `physics_wrecking_ball`, `physics_seesaw`, `physics_pile`, `physics_dominoes`, `physics_bridge`, `physics_barrel_run`, `physics_turntable` | Physics and rigid bodies | ✅ proving ground: a 30-crate tower standing still, 250 crates, drums and balls raining into a bin, a domino run, crash test dummy ragdolls on stairs, a wrecking ball on a chain, a seesaw catapult, a paddle wheel, a plank bridge, a turntable with a rider, oil drums rolling flat and straight down a 47 m mesh runway into a sand pit; stations replay every 20 to 45 s |

## Water, fire, smoke, weather

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 7 | Realistic water that moves efficiently | `water` (test area), `lake`, `falls` | Water simulation | 🟨 test area live (`water`): a tarn with wind waves, a waterfall, stream and pond that really flow; the valley's `lake` and `falls` still show stand-in water |
| 8 | Water affected by the environment and players (wakes, splashes, rain filling it) | `water` (test area), `lake` | Water simulation | 🟨 test area live (`water`): boat wakes, swimmer and wader ripples, gunfire and depth-charge splashes, blocks floating by density, rain rings and puddles; `lake` next |
| 9 | Water splashes (particles) | `magic_splashes`, `water` | Particles and magic effects | ✅ stones dropping into the Spell court's pond throw a crown, spray, foam and ripples; every splash the water module reports (gunfire, depth charges, dropped blocks in `water`) gets spray too |
| 10 | Fire that spreads through burnable things like grass | `field`, `fire_front`, `fire_trees`, `fire_campfire`, `fire_night` | Fire and smoke | ✅ burning meadow that spreads downwind and regrows |
| 11 | Smoke that reacts to bullets, wind currents and explosions | `range`, `smoke_bullets`, `smoke_range`, `smoke_signal`, `smoke_fans` | Fire and smoke | ✅ smoke range on a 50 s loop: grenades lay a screen and signal smoke, turret bursts punch see-through tunnels, a blast blows a hole, fans push the rest across, a cart drives through, a steam vent bends in the wind |
| 12 | True volumetric clouds with fog | every outdoor viewpoint, `weather_storm`, `stratosphere` | Weather, wind and clouds | ✅ volumetric clouds over the whole valley (fair-weather heaps, storm towers, overcast, fog) that drift with the wind and light up in lightning |
| 13 | Realistic rain that changes the ground (wet, puddles, mud) | `weather_rain`, `footsteps`, `plaza` | Weather, wind and clouds | ✅ rain and splashes; the station yard gets wet and fills puddles with ripples (`weather_rain`), other wet surfaces darken |
| 14 | Temperature, weather and wind simulation; dynamic weather that interacts with the world | `weather`, `weather_station` | Weather, wind and clouds | ✅ moving fronts, temperature and wind; the station's wind sock, anemometer, thermometer, rain gauge and snow stake follow the live weather |
| 15 | Snow that settles on the ground | `weather_snow`, `footsteps` snow strip, mountains | Weather, wind and clouds | ✅ snow falls and piles up on the yard and the grass (`weather_snow`) |

## Ground and footsteps

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 16 | Footsteps in mud, grass, snow, sand, gravel, puddles, wood, metal (sound) | `footsteps` | Audio and footsteps | ✅ trail loop with a hiker, a runner, a dog and a horse; sound rings show each step; the ground reports the trail's surfaces to everyone else |
| 17 | Footprints and tracks pressed into mud and snow | `tracks` (footprint lab) | Planets and terrain | ⬜ strips of mud, snow and sand with stand-in walkers |
| 18 | Destructible walls and barricades (Rainbow Six Siege style) | `fortress` | Destructible terrain and buildings | ✅ breach range: shot drywall, kicked-in barricades, thermite through rebar concrete, blown brick, a watchtower and gate that collapse, a mined cliff that caves in (replays every 20 s) |
| 19 | Voxel digging and building (Valheim style) | `voxel` | Destructible terrain and buildings | 🟨 voxel mound with a pit |

## Living world

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 20 | Procedural plants that differ by species variables | `forest`, the whole valley | Foliage and ecology | ✅ `forest`: oak, birch, pine, spruce, hazel, ferns, grass and flowers, each plant shaped by inherited genes; the valley's own trees switch over next |
| 21 | Chop a tree; new ones grow where water, sun and soil allow | `forest_clearing` (walk the trail: `forest_trail`, `forest_spring`, `forest_burn_scar`, `forest_dry_ridge`) | Foliage and ecology | ✅ a tree at the clearing's edge falls every 25 s and rots into the soil; the clear-cut regrows, the burn scar recovers, ferns crowd the spring, pines win the dry ridge |
| 22 | Plants that need other resources (mineral water) and glowing cave mushrooms | `cave`, `cave_fungi`, `cave_pool` | Foliage and ecology | ✅ glowcaps and lantern caps grow only where the mineral pool and seep reach, glow brighter the better fed they are, and light the cave |
| 23 | Grass that moves in the wind | everywhere | Foliage and ecology (strands: Rendering and lighting) | 🟨 grass patches sway |
| 24 | Dynamic fauna (wildlife that grazes, drinks, flees, flocks) | `meadow`, `meadow_herd` (by the lake shore) | Smart NPC AI | ✅ deer and rabbit herds graze, drink, rest and bolt; birds flock; a wolf pack stalks and hunts (stand-in bodies) |

## Characters and combat

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 25 | Robust animation for humanoids | `paddock`, `paddock_hammer`, `paddock_stairs`, `arena` | Animation system | ✅ paddock: a hammer athlete, a lifter, a shirt reach, and a walker that climbs a ramp, crosses a deck, takes the stairs with IK-planted feet and runs back |
| 26 | Animation for animals | `paddock`, `paddock_gallop`, `paddock_spider`, `meadow` | Animation system | ✅ paddock: a horse that walks, trots, canters and gallops round an oval, and a spider picking its way over rocks |
| 27 | Animation for vehicles (wheels, gear, doors) | `paddock` car loop, `paddock_car`, `track`, `airfield` | Animation system | ✅ paddock: a car with suspension over speed bumps, body roll, steering and spinning wheels |
| 28 | Muscles that flex and jiggle with movement | `paddock`, `paddock_hammer`, `paddock_flex`, `paddock_block` | Animation system | ✅ the hammer athlete's arms, shoulders and back tense on the strike and relax after; the lifter's double-biceps flex |
| 29 | Clothing tension and wrinkles during movement | `paddock`, `paddock_reach`, `yard` | Animation system, Cloth, soft bodies and hair | ✅ paddock: sleeves bunch at the elbows and the shirt stretches across the back on a reach |
| 30 | Cloth: flags, sails, curtains, clothes | `yard` | Cloth, soft bodies and hair | ✅ flags and pennants in the wind, a curtain shot full of holes, one that burns, a cape |
| 31 | Soft bodies (jelly, flesh) and ropes (rope bridge) | `yard` | Cloth, soft bodies and hair | ✅ jelly cubes, a clay block that dents, rope bridges, a swing, a bead curtain, a wrecking ball |
| 32 | Hair and fur for humans and animals | `yard` | Cloth, soft bodies and hair | ✅ a walking mannequin with long hair and a dog with fur |
| 33 | Smart NPC AI that uses the environment (cover, high ground) | `arena`, `arena_cover` | Smart NPC AI | ✅ two squads take cover, climb the platform, flank, suppress and shoot the explosive barrel (stand-in bodies) |
| 34 | NPCs that space the player and block attacks | `arena`, `arena_duel` | Smart NPC AI | ✅ melee fighters take turns instead of ringing you, block, parry, dodge and feint; a master and a veteran duel |
| 35 | NPC skill, reaction time and logic editable per enemy | `arena` | Smart NPC AI | ✅ every fighter's skills (reaction time, accuracy, logic, defense…) are tunable through `npc.set_skill`; head band color shows skill |
| 36 | Titanfall-style movement: wall-run, slide, dash | `parkour`, `parkour_course`, `parkour_play` | Movement and slow motion | ✅ a runner loops sprint, slide under a bar, wall-run, wall-jump, mantle onto a tower, double jump; press P to play it yourself (third-person pilot, lock-on with the middle mouse) |
| 37 | Elite enemies dash in and slice in the blink of an eye | `parkour` duel ring, `parkour_blink` | Movement and slow motion | ✅ an elite dash-strikes a level 1 player in under 0.25 s |
| 38 | Stat-based time perception: a leveled-up player sees enemies in slow motion | `parkour` duel ring, `parkour_focus`, `parkour_counter` | Movement and slow motion | ✅ at level 10 the same attack slows the world to 1/5.5; the player sidesteps and counters |
| 39 | Slow motion when attacking or being attacked (Zoro / Sung Jin-Woo moments) | `parkour` duel ring, `parkour_pack` | Movement and slow motion | ✅ four grunts attack in turn; each attack and each kill plays in slow motion |

## Effects and UI

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 40 | Beautiful magic spells (particles) | `magic`, `magic_spells`, `magic_footsteps`, `magic_impacts` | Particles and magic effects | ✅ Spell court: arcane portal, fireballs and frost bolts hitting a wall, lightning between obelisks, a healing aura, fountain, anvil sparks, torches and fireflies; footprint puffs on mud, snow, sand and water and shot impacts on six materials |
| 41 | Conversation UI between two people | `market_dialogue` | UI system and maps | ✅ portraits that talk and blink, typewriter text, choices (one opens the shop) |
| 42 | Shop UI | `market_shop` | UI system and maps | ✅ buy and sell tabs, item cards, quantity, prices and gold |
| 43 | Inventory UI | `market_inventory` | UI system and maps | ✅ bag grid with drag and drop, equipment, weight, item details |
| 44 | Interactive maps: flat map, 3D globe, galaxy map | `market_map_surface`, `market_map_globe`, `market_map_system`, `market_map_galaxy` | UI system and maps | ✅ one map zooming surface → globe → star system → galaxy, with POIs, waypoint and info cards |
| 45 | HUD | `market_hud` (any zone via `ui.show {"screen": "hud"}`) | UI system and maps | ✅ vitals, compass, minimap, quest, hotbar, slow-motion gauge, world markers; also `market_pause`, `market_settings` |

## Engine-wide

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 46 | Hyper-realistic, beautiful scenery | every viewpoint | Beautiful showcase world (renderer: Rendering and lighting) | 🟨 valley, mountains, village, woods, meadows, a cliff with a waterfall; stylized meshes |
| 47 | Physically based materials and lighting, day and night | `plaza`, `sunset`, `night` | Rendering and lighting | ✅ |
| 48 | 2D and 2.5D games | `gallery2d` | Rendering and lighting | 🟨 diorama, no 2D camera yet |
| 49 | All platforms | build matrix | Game engine and test game | ⬜ |
| 50 | Direct AI integration: an AI can inspect, change and screenshot the running game | `showcase.*` APIs, `engine.describe` | Game engine and test game | ✅ |
| 51 | Intuitive, easy to maintain, modules swappable | `ARCHITECTURE.md`, module guard test | Game engine and test game | ✅ |
