# Showcase test cases

Every test case Tobias asked the engine to show, where it lives in the showcase, which thread
owns it, and whether you can see it yet. Viewpoint keys work with `--tour <key>`, the
`showcase.goto` API and the screenshot tour. A tour of one zone's stops (`--tour
paddock_car,paddock_flex`) loads only that zone's demo, which a headless run in a 16 GB
container needs: all the demos together take ~13.6 GB. `--zones a,b` picks the zones (`all`
loads every one).

The Beautiful showcase world thread keeps this list current. Module threads: your zone is one
line in `zones::spawn_demos` (swap the `ph::…` placeholder for your demo) and one line in
`zones::demo_views` (extra tour stops, with warm-up seconds and API calls on arrive/leave). When
your demo replaces a placeholder, update your rows (status and notes) in the same PR.

**Status:** ✅ live (the module's demo runs there) · 🟨 placeholder (the zone and stand-in props
are built, nothing simulates yet) · ⬜ not visible yet

## Scale and travel

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 1 | Huge scale: walk, drive, fly from the ground to orbit and the Moon without loading screens | `overview`, `tower`, `stratosphere`, `orbit`, `moon` | Planets and terrain | ✅ the valley sits on a full-size Earth (6,371 km); the camera flies from the ground to a 420 km orbit and to the Moon at its real distance with no loading screens, and ground detail streams in by level of detail |
| 2 | Efficient enough for multiplanetary travel (planets, moons, orbits, reference frames) | `spaceport`, `orbit`, `moon` | Planets and terrain | ✅ the valley's Earth is one body of engine_space's solar system: the Sun, the planets and the Moon move on their real Kepler orbits, the Moon and Mars are real terrain at their real places (the `moon` stop aims at the Moon wherever it is that day), the Sun and the Moon's phase keep the weather's clock, and `space.travel` re-anchors the world on another body |
| 3 | Cars with real suspension, tires and drivetrain | `track`, `track_cars`, `track_drift`, `track_jump` | Cars, planes and spaceships | ✅ two detailed sports cars race the oval past the grandstand and pits, the rally car drifts on dirt and a car flies off the kicker |
| 4 | Planes with aerodynamic surfaces | `airfield` (1.2 km runway), `airfield_lineup`, `airfield_rotate` | Cars, planes and spaceships | ✅ a plane takes off and one flies circuits |
| 5 | Spaceships with thrusters and RCS, launching to orbit | `spaceport`, `spaceport_hover`, `spaceport_parked`, `spaceport_launch`, `spaceport_orbit`, `spaceport_reentry`, `spaceport_landing` | Cars, planes and spaceships | ✅ the VTOL spaceplane hops and hovers over the pad, then launches and climbs to a 200 × 202 km orbit around the full-size Earth (`spaceport_launch`, `spaceport_orbit`), and comes home from orbit: deorbit, re-entry, landing burn (`spaceport_reentry`, `spaceport_landing`; test returns_from_orbit_and_lands_on_the_pad, lands within 10 m) |
| 6 | Extremely well built, efficient physics: stacking, joints, ragdolls | `physics`, `physics_ragdolls`, `physics_wrecking_ball`, `physics_seesaw`, `physics_pile`, `physics_dominoes`, `physics_bridge`, `physics_barrel_run`, `physics_rockfall`, `physics_turntable`, `lake_floats` | Physics and rigid bodies | ✅ proving ground: a 30-crate tower standing still, 250 crates, drums and balls raining into a bin, a domino run, ten crash test dummy ragdolls tumbling down stairs together without a joint coming apart (the four nearest the camera articulated), all at rest and asleep within 7 s, a wrecking ball on a chain, a seesaw catapult, a paddle wheel, a plank bridge, a turntable with a rider, oil drums rolling flat and straight down a 47 m mesh runway into a sand pit, 300 crushed granite rocks pouring from a hopper down a steel chute into a heap with a 35° face; stations replay every 20 to 45 s. Far from the player physics costs less (level of detail): past 150 m bodies take substeps twice as long, past 600 m they step every other frame, busy far piles split off and merge back when they come near (906 tumbling rocks: 19.8 ms near, 12.0 ms past 150 m, 6.4 ms past 600 m). On the lake (`lake_floats`), a float test: a post, a raft, a light and a waterlogged crate, a plank, a ball, a drum and a steel ball floating by their real shapes (the crates tip onto an edge, the raft lies flat, the steel ball sinks), dropped again every 40 s |

## Water, fire, smoke, weather

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 7 | Realistic water that moves efficiently | `water` (test area), `water_cascade`, `lake`, `lake_jetty`, `lake_boats`, `lake_gale`, `falls` | Water simulation | ✅ test area (`water`): a tarn with wind waves, a waterfall, stream and pond that really flow, and a tiered cascade tumbling down a crevice in a crag into the tarn (`water_cascade`). Valley: the `lake` is real water with a jetty, moored boats and buoys; the stream runs from its spring over the cliff as a free-falling curtain into a full plunge pool and on into the lake. In a gale (`lake_gale`) the lake whitecaps and streaks with spray while the boats ride it out |
| 8 | Water affected by the environment and players (wakes, splashes, rain filling it) | `water` (test area), `lake` | Water simulation | ✅ test area (`water`): swirling, eddying boat wakes, swimmer and wader ripples, gunfire and depth-charge splashes, blocks floating by density, rain rings and puddles. The valley `lake` has the same interactive surface, boats riding its waves, and spray where the falls land |
| 9 | Water splashes (particles) | `magic_splashes`, `water` | Particles and magic effects | ✅ stones dropping into the Spell court's pond throw up a glassy crown of water (ray traced, with fingers and beads on its rim) that tears open as the jet shoots up, then leave lace foam and ripples that bend the water; every splash the water module reports (gunfire, depth charges, dropped blocks in `water`) gets spray too |
| 10 | Fire that spreads through burnable things like grass | `field`, `fire_front`, `fire_trees`, `fire_campfire`, `fire_plume`, `fire_night` | Fire and smoke | ✅ burning meadow, lit on whichever edge faces the wind, that spreads downwind and regrows; each tour stop starts a fresh fire at its moment |
| 11 | Smoke that reacts to bullets, wind currents and explosions | `range`, `smoke_bullets`, `smoke_range`, `smoke_signal`, `smoke_fans` | Fire and smoke | ✅ smoke range on a 50 s loop: grenades lay a screen and signal smoke, turret bursts punch see-through tunnels, a blast blows a hole, fans push the rest across, a cart drives through, a steam vent bends in the wind |
| 12 | True volumetric clouds with fog | every outdoor viewpoint, `weather_storm`, `stratosphere` | Weather, wind and clouds | ✅ volumetric clouds over the whole valley (fair-weather heaps, storm towers, overcast, fog) that drift with the wind and light up in lightning |
| 13 | Realistic rain that changes the ground (wet, puddles, mud) | `weather_rain`, `footsteps`, `plaza` | Weather, wind and clouds | ✅ rain and splashes; the station yard gets wet and fills puddles with ripples (`weather_rain`), other wet surfaces darken |
| 14 | Temperature, weather and wind simulation; dynamic weather that interacts with the world | `weather`, `weather_station` | Weather, wind and clouds | ✅ moving fronts, temperature and wind; the station's wind sock, anemometer, thermometer, rain gauge and snow stake follow the live weather |
| 15 | Snow that settles on the ground | `weather_snow`, `footsteps` snow strip, mountains | Weather, wind and clouds | ✅ snow falls and piles up on the yard and the grass (`weather_snow`) |

## Ground and footsteps

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 16 | Footsteps in mud, grass, snow, sand, gravel, puddles, wood, metal (sound) | `footsteps` | Audio and footsteps | ✅ trail loop with a hiker, a runner, a dog and a horse; sound rings show each step; the ground reports the trail's surfaces to everyone else |
| 17 | Footprints and tracks pressed into mud and snow | `tracks` (footprint lab) | Planets and terrain | ✅ footprint lab: a hiker, a dog, a horse and a rover leave prints and tracks in strips of mud, snow, sand and grass; rain fills the prints, snowfall covers them; a blast crater, a dug trench and a raised mound |
| 18 | Destructible walls and barricades (Rainbow Six Siege style) | `fortress` | Destructible terrain and buildings | ✅ breach range: shot drywall, kicked-in barricades, thermite through rebar concrete, blown brick, a watchtower and gate that collapse, a mined cliff that caves in (replays every 20 s) |
| 19 | Voxel digging and building (Valheim style) | `voxel`, `voxel_build`, `voxel_build_reach`, `voxel_shelter`, `voxel_shelter_pit`, `voxel_cabin_door`, `voxel_stairs` | Destructible terrain and buildings | ✅ a voxel hill you dig into, and a build mode: a roof piece's ghost snaps onto a cabin with framed doorways, windows and gables, a deck built out to its support limit shows a red ghost where it can't hold, a hoe levels a pad in the rock where a builder puts up a shelter, and digging a pit under it drops it in; buildings are solid, their plank doors swing open, and a builder lays two flights of wooden stairs up to the cabin's deck that a character can climb |
| 52 | Siege vertical play: soft roof and hatches | `fortress_roof`, `fortress_hatches` (breach house north of the watchtower) | Destructible terrain and buildings | ✅ every 20 s the room below fires up through the east roof bay, a charge blows a hole in the middle bay and the boards rain in, thermite cuts the steel hatch out and a sledgehammer smashes the wooden hatch in; it all rebuilds each loop |
| 53 | Siege reinforcement: steel panels braced behind a wall | `fortress_reinforce`, `fortress_reinforce_thermite` (bay 8, the west end of the breach range) | Destructible terrain and buildings | ✅ every 20 s a drywall is shot through, reinforced with steel panels and hydraulic struts, shot again, charged, then burned through with thermite |

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
| 25 | Robust animation for humanoids | `paddock`, `paddock_hammer`, `paddock_face`, `paddock_stairs`, `paddock_footwork`, `paddock_flinch`, `paddock_sprint_start`, `paddock_sprint_stop`, `paddock_stunt`, `paddock_stunt_down`, `paddock_stunt_up`, `footsteps` (trail hiker and runner) | Animation system | ✅ paddock: a hammer athlete, a lifter, a shirt reach, a walker that climbs a ramp, crosses a deck, takes the stairs with IK-planted feet and runs back, an athlete who walks a square facing one way (ahead, sidestepping, backpedalling) with every planted foot held still, a shuttle runner who sprints from a standing start leaning into it, digs in to stop, runs back backwards and sideways while a coach turns on the spot to watch, a sparring partner who flinches from blows to the chin, head, stomach and shoulder, and a stuntman who sprints off a deck, falls braced (arms out, knees bent) and goes limp on the crash mat as a ragdoll, then gets up off his back or his front with hands and knees planted and walks back. Faces blink, glance, grit their teeth under effort and go slack when limp |
| 26 | Animation for animals | `paddock`, `paddock_gallop`, `paddock_dog_trot`, `paddock_dog`, `paddock_spider`, `footsteps` (trail horse and dog) | Animation system | ✅ paddock: a textured horse (real model on the horse rig, mane and tail of hair cards) that walks, trots, canters and gallops round an oval, a furred Labrador that walks, trots and breaks into a rotary gallop with its ears and tail on springs, and a furred red-knee tarantula climbing boulders, palps tapping |
| 27 | Animation for vehicles (wheels, gear, doors) | `paddock` car loop, `paddock_car`, `paddock_doors` | Animation system | ✅ paddock: a detailed concept car (real model) with suspension over speed bumps, body roll, steering and spinning wheels, and a parked one whose doors swing open and shut |
| 28 | Muscles that flex and jiggle with movement | `paddock`, `paddock_hammer`, `paddock_flex`, `paddock_block` | Animation system | ✅ the hammer athlete's arms, shoulders and back tense on the strike and relax after; the lifter's double-biceps flex |
| 29 | Clothing tension and wrinkles during movement | `paddock`, `paddock_reach`, `yard` | Animation system, Cloth, soft bodies and hair | ✅ paddock: sleeves bunch at the elbows and the shirt stretches across the back on a reach |
| 30 | Cloth: flags, sails, curtains, clothes | `yard` | Cloth, soft bodies and hair | ✅ flags and pennants in the wind, a curtain shot full of holes, one that burns, a cape, a wool blanket that drops over a stone ball and heaps on itself (cloth self-collision), and a gaff-rigged dinghy on the lake whose simulated sail drives it |
| 31 | Soft bodies (jelly, flesh) and ropes (rope bridge) | `yard` | Cloth, soft bodies and hair | ✅ jelly cubes, a clay block that dents, rope bridges, a swing, a bead curtain, a wrecking ball |
| 32 | Hair and fur for humans and animals | `yard` | Cloth, soft bodies and hair | ✅ a walking mannequin with long hair and a dog with fur |
| 33 | Smart NPC AI that uses the environment (cover, high ground) | `arena`, `arena_cover` | Smart NPC AI | ✅ two squads take cover, climb the platform, flank, suppress and shoot the explosive barrel (stand-in bodies) |
| 34 | NPCs that space the player and block attacks | `arena`, `arena_duel` | Smart NPC AI | ✅ melee fighters take turns instead of ringing you, block, parry, dodge and feint; a master and a veteran duel |
| 35 | NPC skill, reaction time and logic editable per enemy | `arena` | Smart NPC AI | ✅ every fighter's skills (reaction time, accuracy, logic, defense…) are tunable through `npc.set_skill`; head band color shows skill |
| 36 | Titanfall-style movement: wall-run, slide, dash | `parkour`, `parkour_course`, `parkour_vault`, `parkour_ledge`, `parkour_grapple`, `parkour_zip`, `parkour_ladder`, `parkour_freight`, `parkour_play` | Movement and slow motion | ✅ a runner loops sprint, slide under a bar, wall-run, wall-jump, mantle onto a tower, double jump; a pilot sprints down the vault lane and vaults a steel railing, a concrete barrier and strapped cases without slowing, one hand planted on each top and the legs swung over it; a pilot climbs the tower's caged ladder hand over hand, steps onto the roof, climbs back down and slides the rest; a climber catches a ledge too high to vault, shimmies along it and around its corner and climbs up; a pilot grapples the top of a 9 m tower, is reeled up and climbs over, then jumps into a zip line on the roof and rides it 26 m down to a landing pad, sparks off the glove; at the freight station a pilot sprints down the platform, leaps onto a departing box car, wall-runs along its side, climbs onto the roof and rides it to the end of the track; press P to play it yourself (third-person pilot, F to grapple, lock-on with the middle mouse) |
| 37 | Elite enemies dash in and slice in the blink of an eye | `parkour` duel ring, `parkour_blink` | Movement and slow motion | ✅ an elite dash-strikes a level 1 player in under 0.25 s |
| 38 | Stat-based time perception: a leveled-up player sees enemies in slow motion | `parkour` duel ring, `parkour_focus`, `parkour_counter` | Movement and slow motion | ✅ at level 10 the same attack slows the world to 1/5.5; the player sidesteps and counters |
| 39 | Slow motion when attacking or being attacked (Zoro / Sung Jin-Woo moments) | `parkour` duel ring, `parkour_pack` | Movement and slow motion | ✅ four grunts attack in turn; each attack and each kill plays in slow motion |

## Effects and UI

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 40 | Beautiful magic spells (particles) | `magic`, `magic_spells`, `magic_portal`, `magic_starburst`, `magic_footsteps`, `magic_marks`, `magic_impacts`, `magic_engine` | Particles and magic effects | ✅ Spell court: arcane portal, a standing portal in the gate arch swirling the valley seen through it, starburst fireworks whose stars trail glitter and crackle, fireballs and frost bolts hitting a wall and leaving a glowing scorch and a frost film on it, lightning between obelisks, a healing aura, fountain, anvil sparks that split as they skid, torches and fireflies; footprint puffs on mud, snow, sand and water and shot impacts on six materials (throwing solid rock chips and splinters); a rocket motor test-firing on a stand outside the rim, its plume showing shock diamonds |
| 41 | Conversation UI between two people | `market_dialogue` | UI system and maps | ✅ portraits that talk and blink, typewriter text, choices (one opens the shop) |
| 42 | Shop UI | `market_shop` | UI system and maps | ✅ buy and sell tabs, item cards, quantity, prices and gold |
| 43 | Inventory UI | `market_inventory` | UI system and maps | ✅ bag grid with drag and drop, equipment, weight, item details |
| 44 | Interactive maps: flat map, 3D globe, galaxy map | `market_map_surface`, `market_map_globe`, `market_map_system`, `market_map_galaxy` | UI system and maps | ✅ one map zooming surface → globe → star system → galaxy, with POIs, waypoint and info cards |
| 45 | HUD | `market_hud` (any zone via `ui.show {"screen": "hud"}`) | UI system and maps | ✅ vitals, compass, minimap, quest, hotbar, slow-motion gauge, world markers; also `market_pause`, `market_settings`, and `market_journal` (quests by kind with objectives, rewards and a live map of the place; tracking one moves the HUD tracker and the waypoint) and `market_wheel` (a quick-select wheel over the HUD that slows the world while you pick a weapon, item or ability) and `market_skills` (a skill tree of three branches with ranks, prerequisites, level-gated ultimates and point spending) and `market_crafting` (a crafting bench of alchemy, smithing and tinkering recipes made from the bag, with timed crafts and recipes still to learn) |

## Engine-wide

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 46 | Hyper-realistic, beautiful scenery | every viewpoint | Beautiful showcase world (renderer: Rendering and lighting) | 🟨 valley, mountains, village, meadows of GPU grass blades, a cliff with a waterfall; Foliage's oaks, birches, spruces, pines and hazels in the woods and hedges; dry-stone walls of real rock on virtual geometry |
| 47 | Physically based materials and lighting, day and night | `plaza`, `sunset`, `night` | Rendering and lighting | ✅ |
| 48 | 2D and 2.5D games | `gallery2d` | Rendering and lighting | 🟨 diorama, no 2D camera yet |
| 49 | All platforms | build matrix | Game engine and test game | ⬜ |
| 50 | Direct AI integration: an AI can inspect, change and screenshot the running game | `showcase.*` APIs, `engine.describe` | Game engine and test game | ✅ |
| 51 | Intuitive, easy to maintain, modules swappable | `ARCHITECTURE.md`, module guard test | Game engine and test game | ✅ |
