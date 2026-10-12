# engine_vfx — Particles & VFX

**Owner:** Particles and magic effects. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/particles.md` in the project files.

## Scope

- GPU particle system (compute simulation): emitters as components, effects as JSON assets.
- Forces: gravity, drag, wind (`WindField`), vortices, attractors, noise; depth-buffer collision; sub-emitters and events.
- Rendering: lit and unlit sprites, soft particles (scene depth copy), ribbons/trails, mesh particles, distortion, decals.
- Effects library: magic spells, water splashes, sparks, debris, dust, blood, muzzle flashes.
- Respects `TimeDilation` (spells freeze in slow motion).

## Contracts

Reads: `WindField`, `Gravity`, `TimeDilation`, world events (spawn effects on impacts).

Writes: (rendering only).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- A spell with a glowing core, trailing ribbons and sparks lights its surroundings; 1M particles at 60 fps on a mid GPU.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `vfx.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
