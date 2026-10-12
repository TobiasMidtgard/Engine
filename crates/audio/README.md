# engine_audio — Audio

**Owner:** Audio and footsteps. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/sound.md` in the project files.

## Scope

- Mixer and buses on kira; null backend for headless runs.
- Spatial audio: distance attenuation, doppler, occlusion via physics raycasts, reverb zones.
- Footsteps and impacts from `GroundContact` + `SurfaceMaterial::footstep_sound`; weather ambience; music layers.
- Pitch/tempo follow `TimeDilation` for slow motion.

## Contracts

Reads: `GroundContact`, `NoiseEvent`, `WeatherState`, `TimeDilation`.

Writes: (output only).

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- Footsteps change from grass to mud to snow; rain gets louder with intensity; slow motion lowers the pitch.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `audio.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test.
- A `demo` module with `pub fn spawn_demo(commands: &mut Commands, origin: DVec3)` that builds a small showcase of the feature around `origin`; `games/showcase` places it in its zone.
