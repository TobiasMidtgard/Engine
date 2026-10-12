# engine_vfx — Particles & VFX

**Owner:** Particles and magic effects. **Status:** round 1 shipped: GPU particles, trails,
lightning beams, effect lights, 50+ built-in effects, world-event reactions and the Spell court
showcase zone. Background research: `game-engine-research/particles.md` in the project files.

## Using it

```rust,ignore
use engine_vfx::prelude::*;

// A looping effect that follows its entity (spell in flight, torch, portal).
commands.spawn((Transform::from_xyz(0.0, 1.5, 0.0), ParticleEffect::new("spell.fireball")));

// A one-shot burst that removes its entity when done (impact, splash).
spawn_effect_at(&mut commands, "spell.fireball_impact", hit_point, hit_normal, 1.0);

// Bigger, denser, brighter.
ParticleEffect::new("fire.torch").with_scale(2.0).with_rate(1.5).with_intensity(0.8);

// A glowing ribbon behind anything that moves, and a lightning bolt between two points.
commands.spawn((Transform::default(), Trail::default()));
commands.spawn((Transform::from_translation(a), Beam { target: b, ..Default::default() }));
```

Effects are data. `vfx.effects` lists them, `vfx.describe` prints one as JSON, and
`vfx.define` adds or replaces one while the game runs (instances restart with the new
definition), which is how an AI agent designs a new effect. Files named `*.vfx.json` under
`assets/` load and hot-reload the same way (one effect object or a list).

An effect has emitters (layers). Each emitter says when it spawns (`rate`, `rate_per_meter`,
`bursts`, `delay`, `duration`), where (`shape`: point, sphere, hemisphere, cone, ring, disc,
box, line), how particles move (`speed`, `velocity`, `orbit_speed`, `gravity`, `drag`, `wind`,
`turbulence`, `vortex`, `attract`, `collide` with the ground or water: bounce, die or stick) and
how they look (`sprite`: glow, spark, smoke, flame, droplet, ring, star, rune, shard, dust;
`facing`: camera, velocity or horizontal; `color` and `size_over_life`; `emissive` in cd/m²;
`lit` by sun, sky and point lights; `additive`; `soft`). An effect can also carry a point
`light` with flicker and fade, a `trail`, and `prewarm` so loops look settled from the first
frame.

Effects are authored for flat ground in sea-level air and adapt to where they play: gravity
comes from `Env::gravity`, buoyancy (`acceleration`) turns to the local up on a planet, and
buoyancy, `drag` and `turbulence` scale with the air's density from `Env::air`. Smoke barely
rises on Mars, and dust kicked up on the Moon flies in clean ballistic arcs.

## Built-in effects

| Family | Effects |
|---|---|
| Spells | `spell.fireball`, `spell.fireball_impact`, `spell.frost_bolt`, `spell.frost_impact`, `spell.arcane_portal`, `spell.heal_aura`, `spell.charge`, `spell.lightning_hit`, `magic.orb` |
| Water | `water.splash`, `water.splash_small`, `water.fountain` |
| Impacts | `impact.<surface>` for every surface material (dirt, rock, concrete, wood, sand, grass, leaves, snow, mud, water, flesh, ice, glass, metal, default), `impact.sparks`, `impulse.dust`, `explosion.large` |
| Footsteps | `step.dirt`, `step.sand`, `step.snow`, `step.mud`, `step.puddle`, `step.water`, `step.leaves`, `step.default` |
| Other | `muzzle.flash`, `fire.torch`, `ambient.fireflies` |

## Automatic reactions

The module reads `engine_world` messages and plays effects without any game code
([`VfxReactions`] switches each off and caps them per frame):

- `GroundContact`: footsteps and wheels kick up `step.*` by surface, puddles, snow cover and
  water depth.
- `ProjectileTrace`: a splash where a shot crosses water, `SurfaceMaterial::impact_effect` where
  it lands.
- `WorldImpulse`: a dust ring on the ground or a burst on water.
- `WaterSplash`: spray sized by the water displaced.

## APIs

`vfx.effects`, `vfx.describe`, `vfx.define`, `vfx.spawn`, `vfx.stop`, `vfx.clear`, `vfx.beam`,
`vfx.stats`. Tunables: `VfxSettings` (particle budget, density, draw distance, soft particles)
and `VfxReactions`. Components: `ParticleEffect`, `Trail`, `Beam`.

## How it works

- **CPU** ([`emitter`]): each effect instance decides how many particles each emitter spawns
  this frame and where in the pool they go. Each emitter owns a ring of slots in one shared
  pool; new particles overwrite the oldest. Nothing is read back from the GPU.
- **GPU** ([`feature`], `shaders/`): a compute pass moves, collides and relights every live
  particle, a second writes the new ones (spread along the emitter's path so fast spells leave
  a continuous stream). Lit particles get sun (with shadows and forward scattering for smoke),
  sky and point lights, refreshed every fourth frame. Sprites are procedural, drawn as one
  instanced quad per slot, far to near, softened against the scene depth.
- **Precision**: world-space particles are stored relative to the camera and shifted when it
  moves, so effects stay sharp anywhere in a solar system.
- **Time**: everything runs on world time. `TimeDilation` slows spawning and motion together,
  and an entity's `LocalTimeScale` speeds up or freezes its own effects.

## Contracts

Reads: `WindField`, `Gravity`, `Ground`, `Water`, `SurfaceLayers` (puddles, snow),
`SurfaceMaterials`, `TimeDilation`/`LocalTimeScale`, and the messages above.
Writes: rendering only (plus the point lights of effects). The showcase demo writes
`GroundContact` and `WorldImpulse` like a game would.

## Showcase: the Spell court

`demo::spawn_demo` builds the test area in the showcase's `magic` zone, and `demo::VIEWS` lists
its tour stops (`magic_spells` at dusk, `magic_splashes`, `magic_footsteps` and
`magic_impacts`): an arcane portal with orbiting orbs, fireballs and frost bolts hitting a
wall, lightning between two obelisks, a healing aura, a fountain, an anvil throwing sparks, a
pond with stones splashing in, footsteps over mud, snow, sand and water, shot impacts on six
materials, torches and fireflies.

## Next

GPU sub-emitters (spawn on collision or death), depth-buffer collision, sorting within an
emitter, distortion (heat haze), decals, mesh particles, LOD, and a 1M-particle benchmark.

## Checklist

- [x] Plugin registers its tunable components/resources (with docs) and `vfx.*` API functions.
- [x] Honors `TimeDilation`/`LocalTimeScale`.
- [x] Rendering through `RenderFeature`; shaders validated in a unit test.
- [x] Unit tests plus a headless screenshot test.
- [x] A `demo` module with `spawn_demo(commands, origin)`, placed in the showcase.
