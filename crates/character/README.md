# engine_character

People with believable faces: a morphable human model you shape with sliders, drawn with skin,
eye and hair shading meant for close-ups.

- **Shape.** MakeHuman's base mesh (hm08) and its shape targets, released as CC0, baked into
  one pack (`tools/bake_makehuman.py`, fetched to `assets/fetched/character/human_hm08.bin` by
  `cargo run -p fetch-assets -- character`). Sex, age (1..90 years), muscle, weight, height,
  proportions and ancestry blend MakeHuman's macro targets exactly as MakeHuman does; ~150 face,
  neck and torso sliders (`character.sliders`) move the nose, eyes, mouth, jaw, cheeks, ears
  and brows. The surface is subdivided twice (Catmull-Clark) for close-ups.
- **Skin.** Colored by melanin and blood (a ramp through measured skin tones; flushed cheeks,
  nose and ears), with lips, eyelids, beard shadow, freckles and moles from region masks, and
  pores, fine grain, lip lines and age wrinkles computed in the shader. Lighting imitates light
  diffusing under the skin (softer, redder wrap and shadow edges; ears glow when backlit) with
  two highlight lobes.
- **Eyes.** A sphere with a cornea; the iris is seen refracted through it and drawn from its
  anatomy (fibers, crypts, collarette, limbal ring), colored by melanin and lipochrome.
- **Hair.** Strands grown on the skin: brows in a herringbone, lashes along the lid rims,
  beards (stubble, short, full, goatee) and scalp hair (buzz, short, medium, long, bun) with
  clumping, curls and occlusion inside the hair.

## Use

```rust,ignore
app.add_plugins(engine_character::CharacterPlugin);
commands.spawn((Transform::default(), Appearance::preset("Kofi").unwrap()));
```

Change the entity's `Appearance` (or call `character.set`) and it is rebuilt in the background.
`Appearance::random(seed)` gives a plausible random person; `CharacterQuality` picks bust or
head and how smooth.

## The creator

`cargo run -p character_creator --release` opens the creator (`games/character_creator`): a
portrait studio with presets, a random button and tabs of sliders for body, skin, eyes, hair and
face. `-- --headless --shots <dir>` renders portrait sheets; `--views eyes,masks,normals`
adds close-ups and debug views. `cargo run -p engine_character --example dump_strands --
<Preset> out.json` dumps the grown hair for plotting.

## Contracts

Reads nothing from other modules yet. Writes nothing to `engine_world`. It draws through its
own `RenderFeature` (`character`).

## API

`character.list`, `character.get`, `character.set` (partial patches), `character.preset`,
`character.random`, `character.sliders`, `character.debug` (masks, normals, curvature).

## Without the pack

CI and fresh checkouts don't fetch art: the module logs once and draws nothing; every test
uses small packs built in code.
