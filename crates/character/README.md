# engine_character

People with believable faces: a morphable human model you shape with sliders, drawn with skin,
eye and hair shading meant for close-ups.

- **Shape.** MakeHuman's base mesh (hm08) and its shape targets, released as CC0, baked into
  one pack (`tools/bake_makehuman.py`, fetched to `assets/fetched/character/human_hm08_v2.bin`
  by `cargo run -p fetch-assets -- character`) with MakeHuman's default skeleton weights and
  its 34 facial expression units. Sex, age (1..90 years), muscle, weight, height,
  proportions and ancestry blend MakeHuman's macro targets exactly as MakeHuman does; ~150 face,
  neck and torso sliders (`character.sliders`) move the nose, eyes, mouth, jaw, cheeks, ears
  and brows. The surface is subdivided twice (Catmull-Clark) for close-ups.
- **Skin.** Colored by melanin and blood (a ramp through measured skin tones; flushed cheeks,
  nose and ears), with lips, eyelids, beard shadow, freckles and moles from region masks, and
  pores, fine grain, lip lines and age wrinkles computed in the shader. Lighting imitates light
  diffusing under the skin (softer, redder wrap and shadow edges; ears glow when backlit) with
  two highlight lobes.
- **Eyes.** A sphere with a cornea; the iris is seen refracted through it and drawn from its
  anatomy (fibers, crypts, collarette, limbal ring), colored by melanin and lipochrome. The
  socket, brow and lids hide much of the sky from the eye and the upper lid shades it from a
  high sun, so the whites don't glare outdoors.
- **Hair.** Strands grown on the skin: brows in a herringbone, lashes along the lid rims,
  beards (stubble, short, full, goatee) and scalp hair (buzz, short, medium, long, bun) with
  clumped locks, curls (coily short hair stands up rather than combing flat), a soft hairline
  of fine hairs, flyaways and occlusion inside the hair. Long and medium hair is settled by a
  short simulation when the person is built (gravity, bending, collisions with the skin, crown
  hair layered over the hair below it), so it lies on the head and drapes over the shoulders. Shaded with Marschner's fiber model
  (Karis's fit) plus light scattered between fibers.
- **Body and pose.** The whole body (`CharacterBuild::default()`; `portrait()` for a bust),
  dressed in a crew-neck shirt and belted trousers that are a layer of their own over the
  skin (`clothes.rs`: trouser legs hang from the knees, the shirt from the chest, sleeves are
  loose, with folds at the elbows, wrists, waist, knees and hems), and leather shoes built as a
  shell over each foot (`shoes.rs`). It is skinned on the GPU to anim's
  `SkeletonPose` by the humanoid bone names: MakeHuman's bones are folded onto anim's 22
  (`tools/bake_makehuman.py`), each limb is turned from the person's own rest direction to
  anim's, and the feet land where the pose puts them, so a short or tall person follows any
  clip. Hair, brows and lashes ride the skin; motion vectors use last
  frame's pose.
- **Expressions.** 32 face channels (blinks, squints, brows, sneer, jaw, smiles, frown,
  pucker, funnel...) blended from the expression units by ancestry, split into left and right
  halves, applied as sparse morphs on the GPU and driven by `FaceControls` (ARKit names) plus
  gaze; lids follow the eyes down.
- **Mouth.** Teeth (fourteen per jaw, fitted to each person's dental arch), gums and a tongue,
  moving with the jaw and lips. The lips are outlined per pixel in rest space (a crisp
  vermilion border with a cupid's bow and a pale raised edge, philtrum ridges above, moist
  inner band, shadowed corners), and close at rest instead of showing the teeth through a slit.

## Use

```rust,ignore
app.add_plugins(engine_character::CharacterPlugin);
commands.spawn((Transform::default(), Appearance::preset("Kofi").unwrap()));
```

Change the entity's `Appearance` (or call `character.set`) and it is rebuilt in the background.
`Appearance::random(seed)` gives a plausible random person; `CharacterBuild` (or the shared `CharacterQuality` override) picks bust or
head and how smooth.

## The creator

`cargo run -p character_creator --release` opens the creator (`games/character_creator`): a
portrait studio with presets, a random button and tabs of sliders for body, skin, eyes, hair and
face. `-- --headless --shots <dir>` renders portrait sheets; `--views eyes,masks,normals`
adds close-ups and debug views, `--views body,body34 --pose walk` (also `wave`, `think`,
`turn`) the whole person posed, `--views feet` the shoes, and `--views smile,laugh34`
expressions (`neutral`, `smile`, `laugh`, `sad`, `surprise`, `angry`, `disgust`, `blink`,
`look`). `--daylight` shoots outdoors under the midday sky instead of the studio. `cargo run -p engine_character --example dump_strands -- <Preset> out.json` dumps the
grown hair for plotting.

## Contracts

A person's look is the shared `engine_world::character::Appearance` (this crate adds
`AppearanceExt`: sliders by key, presets, random people). It reads:

- `Appearance`: builds and draws the person.
- `AppearanceRequest`: fills in an `Appearance` from the seed, sex, age range and ancestry
  (people with the same `family` seed share ancestry and lean toward one coloring and face).
- `CharacterQuality`: picks the detail (close-up, mid, crowd) unless a `CharacterBuild` is set.

- `SkeletonPose` (anim's humanoid bone names): poses the body; without it the person stands in
  their rest pose.
- `FaceControls`: expressions and gaze.

The outfit (`Appearance::outfit`) sets the clothes' colours and cut: `shirt`, `tshirt` (sleeves to mid upper arm) or `none` (bare to the waist; women wear a band across the chest) on top, `trousers`, `shorts` (mid-thigh) or a `skirt` (to above the knees) below, and an accent colour on the `collar` or an armband (`armband_l`, `armband_r`).

Once a person is built it writes `BodyFit` (their rest skeleton on the humanoid bones and standing height), and rewrites it when the build changes it, so posers can fit their skeletons and props to that person.

Headgear (`Appearance::headgear`) leaves out what a helmet covers, for whoever draws the helmet: `open_helm` the hair, ears and neck (the face from brow to chin stays), `closed_helm` the whole head and neck. A `hood` is drawn here, in the top's colour: loose over the head and ears, open around the face, hanging from the crown and settling onto the shirt as a cowl, over the scalp hair it hides.

It calls `draws_appearances`, so anim poses people with an `Appearance` without drawing its
own mesh for them. It writes `HairGuides` for posed people whose hair moves, and draws each
hair blended from its three nearest guides as the hair simulation moves them.
It curls each hand's fingers by its `HandControls` grip (0 open .. 1 a fist): the pack has no
finger bones, so the skin shader bends the fingers at their joints and the thumb across the palm
(`hands.rs`).
It draws through its own `RenderFeature` (`character`).

## API

`character.list`, `character.get`, `character.set` (partial patches), `character.preset`,
`character.random`, `character.sliders`, `character.debug` (masks, normals, curvature).

## Without the pack

CI and fresh checkouts don't fetch art: the module logs once and draws nothing; every test
uses small packs built in code.
