# engine_ui — UI

**Owner:** UI. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/ui.md` in the project files.

## Scope

- Retained UI with flexbox layout, high-quality text, theming, animation/tweens, gamepad navigation; draws via the renderer's overlay hook.
- Screens: two-person conversation (portraits, typewriter text, choices), shop, inventory (grid, drag and drop, tooltips), HUD, menus.
- Interactive maps: flat map, 3D spherical planet map, galaxy map.
- Layouts as data (JSON) so designers and AI can edit them; data binding to ECS.

## Contracts

Reads: engine_input actions and mouse, ECS state for bindings.

Writes: UI events (clicked, chose dialogue option).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- A dialogue with choices, a shop and inventory with drag and drop, and a map that zooms from a planet globe to the galaxy.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `ui.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test; a scene in `games/showcase`.
