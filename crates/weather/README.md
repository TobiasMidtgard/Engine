# engine_weather — Weather & day–night

**Owner:** World simulation. **Status:** stub. The owning workstream replaces this with the real module.
Background research: `game-engine-research/lighting.md` in the project files.

## Scope

- Weather simulation (fronts, pressure, humidity, temperature with altitude and time of day), seasons.
- Drives `WeatherState`, `WindField`, cloud cover and precipitation; lightning.
- Rain and snow: particles near the camera, surface wetness, puddles, snow accumulation and melt in `SurfaceLayers` (the renderer already darkens wet surfaces and whitens snowy ones from these).
- Day–night cycle: sun and moon `DirectionalLight` direction/illuminance/color, `FogSettings`, star visibility.

## Contracts

Reads: `Time`, Atmosphere/planet (latitude, axial tilt from engine_space), HeatField (fires warm the air).

Writes: `WeatherState`, `WindField`, `SurfaceLayers`, `DirectionalLight`, `FogSettings`.

Cross-module interaction goes through `engine_world` (see AGENTS.md). If a contract is missing, ask the core owner to add it rather than depending on another module's crate.

## Done when

- A storm rolls in: wind rises, clouds thicken, rain wets the ground and fills puddles, then clears to a sunset.

## Checklist

- Plugin registers its tunable components/resources (with docs) and `weather.*` API functions.
- Fixed-step simulation in the right `SimSet`; honors `TimeDilation`/`LocalTimeScale`.
- Rendering through `RenderFeature`; shaders validated in a unit test.
- Unit tests plus a headless screenshot test; a scene in `games/showcase`.
