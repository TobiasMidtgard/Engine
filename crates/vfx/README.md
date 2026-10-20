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
how they look (`sprite`: glow, spark, smoke, flame, droplet, ring, star, rune, shard, dust,
haze, billow, vortex, plume, crown, ripple, foam, blaze; `facing`: camera, velocity, horizontal or axis (standing along the
emitter's direction); `color` and `size_over_life`; `emissive` in cd/m²;
`lit` by sun, sky and point lights; `additive`; `soft`), or `debris` to draw solid 3D pieces
instead of sprites ([`Debris`]: rock, splinter, shard or scrap). An effect can also carry a point
`light` with flicker and fade, a `trail`, and `prewarm` so loops look settled from the first
frame.

Layers can feed each other on the GPU (sub-emitters, [`SubEmit`]): `on_hit` spawns particles
in another layer where a particle hits the ground, water or something on screen (turned out
of the surface), `on_death` where it dies, and `trailing` drops `count` a second along each
particle's path. The fed layer has no spawning of its own; give `count`, `chance` and
`inherit_velocity`. Metal sparks split as they skid this way, and `spell.starburst` is a shell
that bursts into stars, each trailing glitter and crackling as it burns out. Sprites narrower
than 1.5 pixels are drawn 1.5 pixels wide and dimmed to match, so distant sparks still show.

Effects are authored for flat ground in sea-level air and adapt to where they play: gravity
comes from `Env::gravity`, buoyancy (`acceleration`) turns to the local up on a planet, and
buoyancy, `drag` and `turbulence` scale with the air's density from `Env::air`. Smoke barely
rises on Mars, dust kicked up on the Moon flies in clean ballistic arcs, and an engine's plume
loses its shock diamonds and balloons out as the air thins toward space.

Engine exhausts (`exhaust.*`) play at a nozzle with the effect's +Y pointing down the exhaust;
`scale` 1 is the nozzle size each one names and `intensity` follows the throttle. Every nozzle
of an `Exhausts` component (written by vehicles, jump kits) gets its kind's effect by itself,
sized by its radius and following its level; running jets and tailpipes keep idling while the
entity's `EngineState` says the engine runs, and each runs on its owner's `LocalTimeScale`
([`exhaust`]). A cut engine's smoke drifts on, and it relights without starting over. The
flame and nozzle glow are `steady` layers, which keep their full rate whatever the effect's
`rate` and the density setting (stopping only at rate 0), so they never thin into flicker. The `plume`
sprite draws the flame procedurally: a white-hot core, a train of shock diamonds about a
nozzle's width apart with the gas banded bright and dark around them, and a turbulent tail
that flickers and tears away.

Fires burn as one body, not a heap of paper cut-outs. The `blaze` sprite draws a whole fire
standing on its fuel: a churning, domain-warped body that sways, breaks into a few tongues at
the top and colors itself by heat, deep red at the rim, the layer's color in the body and near
white only in the core. `fire.torch` stands a few overlapping blazes on the coals (`steady`, in
its own space) and lets narrower flame tongues, tips and embers rise out of them.

Splashes are water, not white puffs. The `crown` sprite ray traces the splash's crown inside
its square: a wall of water thrown up in a ring that flares out, its rim drawn into fingers
with a bead on each tip, then tears open and slumps while the jet shoots up from the middle. It
mirrors the sky at a glance, glints in the sun and whitens only where it froths. `ripple`
bends the water behind it in a train of wavelets and catches the sky on their crests, and
`foam` floats a lace of bubbles that breaks up over a few seconds (an even whiteness when too
far away to show its bubbles). Both ride the surface in the effect's own space, so the air
neither holds them up nor lets them sink.

## Built-in effects

| Family | Effects |
|---|---|
| Spells | `spell.fireball`, `spell.fireball_impact`, `spell.frost_bolt`, `spell.frost_impact`, `spell.arcane_portal`, `spell.portal_gate`, `spell.heal_aura`, `spell.charge`, `spell.lightning_hit`, `spell.starburst`, `magic.orb` |
| Water | `water.splash`, `water.splash_small`, `water.fountain`, `water.column`, `water.bow_spray`, `water.mist` |
| Impacts | `impact.<surface>` for every surface material (dirt, rock, concrete, wood, sand, grass, leaves, snow, mud, water, flesh, ice, glass, metal, default), `impact.sparks`, `impulse.dust`, `explosion.large` |
| Footsteps | `step.dirt`, `step.sand`, `step.snow`, `step.mud`, `step.puddle`, `step.water`, `step.leaves`, `step.default` |
| Engines | `exhaust.rocket`, `exhaust.afterburner`, `exhaust.jet`, `exhaust.thruster`, `exhaust.tailpipe`, `exhaust.backfire`, `exhaust.jumpjet` |
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

## Marks

[`Marks`] holds what effects leave behind, drawn by [`MarksFeature`] as boxes projected onto
whatever is under them (terrain, walls, props), so nothing needs UVs:

- **Scorch**: a charred blast mark with ash flecks where a spell hit; embers glow in its cracks
  and cool over `glow_secs`.
- **Frost**: a pale film with crystal feathers over the surface, melting back from its edges.
- **Wet**: splashed stone that darkens and dries from the edges.
- **Chip**: a pock with cracks and a dust ring where a shot hit stone or concrete.

An effect leaves one with `mark` in its [`EffectDesc`] (`spell.fireball_impact`,
`spell.frost_impact`, `impact.rock`, `impact.concrete`); games and scripts add their own with
[`Marks::add`] or `vfx.mark`. Marks fade with age and distance and follow world shifts; the
newest 2048 are kept, and a mark landing on top of one of its kind replaces it. Footprints
are not marks: the terrain module presses every `GroundContact` into its ground, and burning
leaves the fire module's own scorch. `cargo run -p engine_vfx --release --example marks`
renders every kind.

## APIs

`vfx.effects`, `vfx.describe`, `vfx.define`, `vfx.spawn`, `vfx.stop`, `vfx.clear`, `vfx.beam`,
`vfx.mark`, `vfx.stats`. Tunables: `VfxSettings` (particle budget, density, draw distance, soft particles)
and `VfxReactions`. Components: `ParticleEffect`, `Trail`, `Beam`.

## How it works

- **CPU** ([`emitter`]): each effect instance decides how many particles each emitter spawns
  this frame and where in the pool they go. Each emitter owns a ring of slots in one shared
  pool; new particles overwrite the oldest. Nothing is read back from the GPU.
- **GPU** ([`feature`], `shaders/`): a compute pass moves, collides and relights every live
  particle, a second writes the new ones (spread along the emitter's path so fast spells leave
  a continuous stream). Particles that hit, die or fly on for a sub-emitter append events,
  and a third pass turns them into particles of the fed layer, claiming slots in its ring
  with an atomic cursor (up to 65 536 events a frame). Lit particles get sun (with shadows,
  and for smoke forward scattering over a floor for the light a thick puff scatters many
  times), sky and point lights, refreshed every fourth frame. A last pass sorts each visible layer's
  particles by distance (a bitonic sort per run of 2048 slots), so nearer puffs of smoke
  cover farther ones. Sprites are procedural, drawn as one instanced quad per particle, layers
  and particles far to near, softened against the scene depth.
- **Debris** ([`debris`]): a `debris` layer draws each particle as a small flat-shaded mesh
  (four variants of each kind) tumbling about an axis of its own, in the depth prepass, the
  opaque pass and the two nearest shadow cascades, so chips are lit, shadowed and motion
  blurred like the rest of the scene. They rest on the ground plane and fade by dissolving.
- **Precision**: world-space particles are stored relative to the camera and shifted when it
  moves, so effects stay sharp anywhere in a solar system.
- **Time**: everything runs on world time. `TimeDilation` slows spawning and motion together,
  and an entity's `LocalTimeScale` speeds up or freezes its own effects.

## Contracts

Reads: `WindField`, `Gravity`, `Ground`, `Water`, `SurfaceLayers` (puddles, snow),
`SurfaceMaterials`, `Exhausts` and `EngineState`, `TimeDilation`/`LocalTimeScale`, and the
messages above. Inserts `ExhaustEffects` (`draws_exhausts`), so owners hide their stand-in
plumes.
Writes: rendering only (plus the point lights of effects). The showcase demo writes
`GroundContact` and `WorldImpulse` like a game would.

## Showcase: the Spell court

`demo::spawn_demo` builds the test area in the showcase's `magic` zone, and `demo::VIEWS` lists
its tour stops (`magic_spells` at dusk, `magic_portal`, `magic_starburst`, `magic_splashes`,
`magic_footsteps`, `magic_marks`, `magic_engine` and `magic_impacts`): an arcane portal with orbiting orbs before
a gate whose arch holds a standing portal swirling the valley seen through it, a mortar firing
starburst fireworks over the court, fireballs and frost bolts scorching and frosting a wall,
lightning between two obelisks, a healing aura, a fountain, an anvil throwing sparks that
split as they skid, a pond with stones splashing in, a golem stomping over mud, snow, sand and
water, shot impacts on six materials (chipping the rock), and braziers burning charcoal around
the rim, with fireflies in the shrubs beyond. Outside the north-east rim a rocket motor on a
stone test stand fires every eleven seconds: an igniter's burst of sparks, then ten seconds of
plume with shock diamonds and a cloud of exhaust rolling away. Its glowing stone (runes, crystals, coals)
follows the camera's exposure as particle glow does ([`VfxSettings::glow_exposure`]), so it
keeps its color at night instead of turning white.
`cargo run -p engine_vfx --release --example sub_emitters` renders the firework and the
splitting sparks; `--example braziers` renders the brazier fires, `--example engines` every
exhaust side on (add `vacuum` to take the air away), `--example splashes` the splashes close up
as they unfold, and `--example court` the whole court, each by day, at dusk and at night.

## Next

LOD, and a 1M-particle benchmark on a real GPU.

## Checklist

- [x] Plugin registers its tunable components/resources (with docs) and `vfx.*` API functions.
- [x] Honors `TimeDilation`/`LocalTimeScale`.
- [x] Rendering through `RenderFeature`; shaders validated in a unit test.
- [x] Unit tests plus a headless screenshot test.
- [x] A `demo` module with `spawn_demo(commands, origin)`, placed in the showcase.
