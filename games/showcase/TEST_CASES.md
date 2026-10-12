# Showcase test cases

Every test case Tobias asked the engine to show, where it lives in the showcase, which thread
owns it, and whether you can see it yet. Viewpoint keys work with `--tour <key>`, the
`showcase.goto` API and the screenshot tour.

The Beautiful showcase world thread keeps this list current. Module threads: when your demo
replaces a placeholder, update your rows (status and notes) in the same PR.

**Status:** ✅ live (the module's demo runs there) · 🟨 placeholder (the zone and stand-in props
are built, nothing simulates yet) · ⬜ not visible yet

## Scale and travel

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 1 | Huge scale: walk, drive, fly from the ground to orbit and the Moon without loading screens | `overview`, `tower`, `stratosphere`, `orbit`, `moon` | Planets and terrain | 🟨 camera flies there; planet surface from orbit is flat color |
| 2 | Efficient enough for multiplanetary travel (planets, moons, orbits, reference frames) | `spaceport`, `orbit`, `moon` | Planets and terrain | ⬜ |
| 3 | Cars with real suspension, tires and drivetrain | `track` | Cars, planes and spaceships | 🟨 |
| 4 | Planes with aerodynamic surfaces | `airfield` (1.2 km runway) | Cars, planes and spaceships | 🟨 |
| 5 | Spaceships with thrusters and RCS, launching to orbit | `spaceport` | Cars, planes and spaceships | 🟨 |
| 6 | Extremely well built, efficient physics: stacking, joints, ragdolls | `physics` | Physics and rigid bodies | 🟨 |

## Water, fire, smoke, weather

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 7 | Realistic water that moves efficiently | `lake` | Water simulation | 🟨 still water stand-in |
| 8 | Water affected by the environment and players (wakes, splashes, rain filling it) | `lake` | Water simulation | ⬜ |
| 9 | Water splashes (particles) | `lake` | Particles and magic effects | ⬜ |
| 10 | Fire that spreads through burnable things like grass | `field`, `fire_front`, `fire_trees`, `fire_campfire`, `fire_night` | Fire and smoke | ✅ burning meadow that spreads downwind and regrows |
| 11 | Smoke that reacts to bullets, wind currents and explosions | `range` | Fire and smoke | 🟨 firing lanes, fans |
| 12 | True volumetric clouds with fog | every outdoor viewpoint, `stratosphere` | Weather, wind and clouds | ⬜ flat cloud layer only |
| 13 | Realistic rain that changes the ground (wet, puddles, mud) | `footsteps`, `plaza` | Weather, wind and clouds | ⬜ (renderer can already darken wet surfaces) |
| 14 | Temperature, weather and wind simulation; dynamic weather that interacts with the world | `weather` hill | Weather, wind and clouds | 🟨 station instruments |
| 15 | Snow that settles on the ground | `footsteps` snow strip, mountains | Weather, wind and clouds | 🟨 painted snow only |

## Ground and footsteps

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 16 | Footsteps in mud, grass, snow, sand, gravel, puddles, wood, metal (sound) | `footsteps` | Audio and footsteps | 🟨 strips built, ground query reports each surface |
| 17 | Footprints and tracks pressed into mud and snow | `footsteps` | Planets and terrain | ⬜ |
| 18 | Destructible walls and barricades (Rainbow Six Siege style) | `fortress` | Destructible terrain and buildings | 🟨 brick wall, barricades |
| 19 | Voxel digging and building (Valheim style) | `voxel` | Destructible terrain and buildings | 🟨 voxel mound with a pit |

## Living world

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 20 | Procedural plants that differ by species variables | `forest`, the whole valley | Foliage and ecology | 🟨 showcase's own procedural trees |
| 21 | Chop a tree; new ones grow where water, sun and soil allow | `forest` clearing | Foliage and ecology | 🟨 stumps, fallen trunk, saplings |
| 22 | Plants that need other resources (mineral water) and glowing cave mushrooms | `cave` | Foliage and ecology | 🟨 mushrooms glow, no growth |
| 23 | Grass that moves in the wind | everywhere | Foliage and ecology (strands: Rendering and lighting) | 🟨 grass patches sway |
| 24 | Dynamic fauna (wildlife that grazes, flees, flocks) | `meadow` | Smart NPC AI | 🟨 deer and hare stand-ins |

## Characters and combat

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 25 | Robust animation for humanoids | `stage`, `arena` | Animation system | 🟨 mannequins |
| 26 | Animation for animals | `stage`, `meadow` | Animation system | 🟨 |
| 27 | Animation for vehicles (wheels, gear, doors) | `stage` cart, `track`, `airfield` | Animation system | 🟨 |
| 28 | Muscles that flex and jiggle with movement | `stage` | Animation system | ⬜ |
| 29 | Clothing tension and wrinkles during movement | `stage`, `yard` | Animation system, Cloth, soft bodies and hair | ⬜ |
| 30 | Cloth: flags, sails, curtains, clothes | `yard` | Cloth, soft bodies and hair | 🟨 rigid flags and curtain |
| 31 | Soft bodies (jelly, flesh) and ropes (rope bridge) | `yard` | Cloth, soft bodies and hair | 🟨 |
| 32 | Hair and fur for humans and animals | `yard` | Cloth, soft bodies and hair | 🟨 stand-in person and dog |
| 33 | Smart NPC AI that uses the environment (cover, high ground) | `arena` | Smart NPC AI | 🟨 |
| 34 | NPCs that space the player and block attacks | `arena` | Smart NPC AI | 🟨 |
| 35 | NPC skill, reaction time and logic editable per enemy | `arena` | Smart NPC AI | ⬜ |
| 36 | Titanfall-style movement: wall-run, slide, dash | `parkour` | Movement and slow motion | 🟨 walls, platforms, slide tunnel |
| 37 | Elite enemies dash in and slice in the blink of an eye | `arena` | Movement and slow motion | ⬜ |
| 38 | Stat-based time perception: a leveled-up player sees enemies in slow motion | `arena` | Movement and slow motion | ⬜ |
| 39 | Slow motion when attacking or being attacked (Zoro / Sung Jin-Woo moments) | `arena` | Movement and slow motion | ⬜ |

## Effects and UI

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 40 | Beautiful magic spells (particles) | `magic` | Particles and magic effects | 🟨 rune circle glows |
| 41 | Conversation UI between two people | `market` | UI system and maps | 🟨 two villagers |
| 42 | Shop UI | `market` | UI system and maps | 🟨 stalls |
| 43 | Inventory UI | `market` | UI system and maps | ⬜ |
| 44 | Interactive maps: flat map, 3D globe, galaxy map | `market` map table | UI system and maps | 🟨 table and globe |
| 45 | HUD | any | UI system and maps | ⬜ |

## Engine-wide

| # | Test case | Zone / viewpoint | Owner | Status |
|---|---|---|---|---|
| 46 | Hyper-realistic, beautiful scenery | every viewpoint | Beautiful showcase world (renderer: Rendering and lighting) | 🟨 valley, mountains, village, woods, meadows; stylized meshes |
| 47 | Physically based materials and lighting, day and night | `plaza`, `sunset`, `night` | Rendering and lighting | ✅ |
| 48 | 2D and 2.5D games | `gallery2d` | Rendering and lighting | 🟨 diorama, no 2D camera yet |
| 49 | All platforms | build matrix | Game engine and test game | ⬜ |
| 50 | Direct AI integration: an AI can inspect, change and screenshot the running game | `showcase.*` APIs, `engine.describe` | Game engine and test game | ✅ |
| 51 | Intuitive, easy to maintain, modules swappable | `ARCHITECTURE.md`, module guard test | Game engine and test game | ✅ |
