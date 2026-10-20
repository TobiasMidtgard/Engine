# engine_ui — UI

**Owner:** UI system and maps. **Status:** round 8: every screen below works and has a test
and a gallery screenshot; the showcase's market street is detailed procedural art; quests and
the journal drive the HUD tracker and the map waypoint; a quick-select wheel slows the world
while you choose a weapon; a skill tree spends points on ranks of passive, active and ultimate
skills; a crafting bench turns what is in the bag into potions, gear and gadgets.
Background research: `game-engine-research/ui.md` in the project files.

## What it is

A retained UI of ECS nodes: flexbox layout (taffy), shaped text with markup (cosmic-text), SVG
icons (resvg), theme classes with hover/focus/press states and transitions, mouse, keyboard and
gamepad navigation, and one instanced draw per frame through a `RenderFeature` (shaders are
validated in a unit test). Layouts are data: `El` builders in Rust, or the same tree as JSON
through `ui.spawn` / `ui.show {"screen": "layout"}`. Text binds to data with `{path}`
templates.

UI animation runs on real time, so menus keep moving while the game is paused or in slow
motion; the HUD's time gauge shows `Time::scale()` and the pause menu sets the base time scale
to 0 and restores it when it closes.

## Screens (`ui.show {"screen": ...}`)

| Screen | What it shows | Arguments |
|---|---|---|
| `dialogue` | Two-person conversation: talking, blinking portraits, typewriter text, choices with tags, letterbox | `tree` (id or whole tree), `node`, `speed` |
| `shop` | Buy and sell tabs, rarity-coloured item cards, quantity picker, price and gold | `vendor`, `owner`, `mode`, `select` |
| `inventory` | Bag grid with drag and drop, equipment slots, weight, item details, use, drop, sort | `owner`, `tab`, `select` |
| `hud` | Vitals with portrait, compass with waypoint bearing, minimap, quest tracker, hotbar with cooldowns, weapon, interaction prompt, slow-motion gauge, world markers | |
| `map` | One map that zooms surface → planet globe → star system → galaxy, with POIs, layers, waypoint and info cards | `zoom` (0-4), `system`, `planet`, `select`, `focus` |
| `journal` | Quests by kind (main story, side quests, contracts), active and finished tabs, objectives with counts, rewards, who gave each, a live map of where it leads; track a quest or show its place on the map | `select` (quest id), `tab` |
| `wheel` | Quick-select wheel: weapons, items and abilities around a ring, picked by pointing (mouse, locked-cursor motion or either stick), a click, A, Enter or a number key; slows the world while open; hold-and-release mode; picks write the HUD's `weapon.*` | `items` (default `wheel.items`), `current`, `slow`, `hold`, `hold_pad` |
| `skills` | Skill tree (`SkillTree`): branches of passive, active and ultimate skills linked by prerequisites, with ranks, point costs and level gates; a details panel with every rank and what each needs; learn with the button, Space or X, reset (after a confirm) with R or Y; learning writes `skills.<id>` and `player.skill_points` and sends `skill.learned:<id>`; `skills.tree`, `skills.learn`, `skills.grant` and `skills.reset` drive it | `select` (skill id) |
| `crafting` | Crafting bench (`RecipeBook`): alchemy, smithing and tinkering recipes with what each makes, its stats and the ingredients it needs against what the bag holds; unknown recipes show as silhouettes with where to learn them; choose how many and craft, a timed build-up in a ring of light, then ingredients leave the bag and the result lands in it (`craft.made:<id>`); `craft.recipes`, `craft.make` and `craft.learn` drive it | `station`, `select` (recipe id), `owner` |
| `pause` | Pause menu with a character card; pauses game time | |
| `settings` | Gameplay, Video (`RenderSettings`), Audio, Controls (rebinding), Interface (scale, accent colour) | `tab` |
| `confirm` | Yes/no dialog that sends `confirm:<action>` | `title`, `text`, `confirm`, `cancel`, `action`, `danger` |
| `layout` | Any JSON layout | `layout` |

Toasts (`ui.notify`) and tooltips sit on their own layers.

## API

- `ui.*`: show, hide, hide_all, screens, tree, click, focus, set_data, get_data, spawn,
  despawn, icons, add_icon, theme, events, notify.
- `dialogue.*`: list, add, state, choose, next.
- `shop.*`: buy, sell, stock. `inventory.*`: give, list, use, move, sort, items.
- `map.*`: data, systems, add_system, add_poi, set_player, waypoint, view, state.
- `hud.*`: marker, markers, remove_marker.
- `quest.*`: log, add, progress, track, accept, complete.

Every call has a summary and an example in the registry (`ui.screens` also lists screen kinds
with example arguments), so an AI agent can drive the whole UI as a player would.

## Quests

`quests::QuestLog` holds every quest (plain JSON: kind, state, level, giver, region, summary,
objectives with counts and map places, rewards) and the tracked one. The tracked quest fills
the HUD tracker's `quest.*` data and moves the map waypoint to its current objective's place,
so the compass, minimap and world markers follow it. Steps after the current one stay hidden
until it is done. Dialogue choices and buttons use the actions `quest.accept:<id>`,
`quest.track:<id>`, `quest.advance:<id>`, `quest.complete:<id>` and `quest.fail:<id>`;
completing a quest gives its gold and items to the Player's inventory and its XP to
`player.xp`.

## Data binding

- `{player.gold}`: a path in `UiData` (`ui.set_data {"path": "player.gold", "value": 120}`);
  `{player.speed:1}` rounds to one decimal.
- `{@Player.Health.current}`: a field of a registered component on a named entity.
- `$RenderSettings.bloom_intensity`: a field of a registered resource (sliders and toggles
  write back through the registry).

## Contracts

Reads: engine_input actions, keys, mouse and gamepads; `Time`; registered components and
resources for bindings; `engine_render::RenderSettings` for the video settings.

Writes: `UiEvent` messages (actions, highlights, drops, tabs, screens opened and closed), also
logged for `ui.events`. Systems read them with a cursor, so actions sent through the API
between frames are handled too.

Cross-module interaction goes through `engine_world` (see AGENTS.md).

## Showcase

`demo::spawn_demo(commands, origin)` fills the demo data (player, merchant, conversations, the
Veil galaxy), builds the market street and pins world markers over Old Brannoc's stall. The
showcase's `market` zone calls it, and its tour opens each screen in turn in front of the
trader (`market_hud`, `market_dialogue`, `market_shop`, `market_inventory`, `market_journal`,
`market_wheel`,
`market_skills`, `market_crafting`, `market_map_*`, `market_pause`, `market_settings`; the camera is `demo::market::ui_view()`).

The market street (`demo::market`): five stalls around a cobbled square (produce, cloth, Old
Brannoc the trader, potter, smith), each with its own striped awning, lantern light and goods;
a mast with bunting to every stall, lamp posts, a well, benches, a handcart, a map table with
a parchment map of Veyra drawn from the map's own terrain, and clutter between the stalls.
Everything is generated in code from `demo::art`, which the gallery's backdrop shares:

- `art::bake_set`: PBR texture sets (albedo, normal from height, occlusion/roughness/metal)
  from per-texel functions in `art::textures` (wood, canvas awnings, metal, dirt, rock,
  cobbles, wicker, linen, the parchment map). Tiled ones repeat without seams (unit test).
- `art::mesh`: rounded boxes and beams, lathes, tubes, crystals, icospheres, all with metric
  UVs. `art::props`: crates, barrels, sacks. `art::stall`: a stall of any trade.

`cargo run -p engine_ui --example market -- <dir> [width] [height] [view ...]` renders the
market alone on a meadow (views: overview, brannoc, produce, cloth, potter, smith, table,
square).

## Tests and screenshots

- `cargo test -p engine_ui`: unit tests, `tests/flows.rs` (mouse click, dialogue choice opens
  the shop, buying, drag and drop, map zoom and waypoint, quests and the journal, the
  quick-select wheel, the skill tree, crafting, pause and resume, key rebinding) and
  `tests/headless.rs` (renders a panel and checks pixels; needs a GPU or lavapipe).
- `cargo run -p engine_ui --example gallery --release -- <dir> [width] [height] [shot ...]`
  renders every screen (hud, wheel, dialogue, dialogue_choices, shop, shop_sell, inventory,
  inventory_drag, journal, journal_finished, skills, crafting, crafting_busy, map_galaxy, map_system, map_globe, map_surface, pause, confirm, settings,
  settings_video, settings_controls, theme_cyan) to PNGs.
