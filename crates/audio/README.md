# engine_audio — Audio

**Owner:** Audio and footsteps. Background research: `game-engine-research/sound.md` in the
project files.

Spatial sound for the engine. Footsteps play real recordings where they have been fetched
(`cargo run -p fetch-assets -- audio`); everything else, and every footstep without them, is
synthesized while it plays, so footsteps never repeat exactly and follow the ground's state:
the same grass sounds different wet, frozen, snowed on or burnt.

## What you hear

- **Footsteps** from every `GroundContact` (feet, paws, hooves, bodies falling). 18 grounds
  have their own sound: grass, mud, snow, ice, gravel, sand, dirt, rock, concrete, wood,
  metal, glass, water, leaves, fabric, flesh, ash and a default; any other
  `SurfaceMaterial` gets a guess from its density and deformation. Heavier and faster steps
  are louder and sharper (a contact without a speed takes its walker's `AnimationParams`
  speed). The ground's state (`SurfaceLayers`) changes them: wet ground
  squelches, puddles splash, snow cover crunches over whatever is under it, burnt ground
  crackles, mud freezes hard below 0 °C, and wading drags through water.
- **Recorded footsteps** on most trail grounds (grass, snow, gravel, wood, metal, concrete,
  sand, leaves) and rock: CC0 and CC-BY field recordings from Freesound
  (`assets/manifests/audio.toml`, about 10 MB) are cut into about 270 single-step takes at
  startup (on a background thread), walking and running ones apart, each levelled to the
  loudness of the synthesized step it replaces. A take ends where the step does (back at the
  recording's noise, at most 0.35 s on hard ground, where its scuff fades out, and 0.6 s on
  soft); jumps, scrapes, odd steps and steps less than 35 dB over the recording's hiss are
  left out, and a ground with fewer than 8 clean takes is synthesized: today mud (one
  recording hissy, the other bright noise), ice and puddles (too much background).
  A step plays a random take (never one of the last few), slightly re-pitched and
  re-levelled, louder and brighter for heavier and faster steps, duller when wet; the
  synthesis adds only what the recording lacks (rain's squelch, a puddle's splash, a heavy
  walker's thump) and takes over when the ground has changed too much (frozen mud) or there
  are no takes (other grounds, paws, hooves, wheels, falling bodies, CI). Deep snow plays
  snow whatever is under it. Rock has its own takes too; tile and asphalt borrow concrete's,
  stone rock's and planks wood's.
- **Impacts** from physics contacts (what hit what, how hard), **splashes** from
  `WaterSplash`, **explosions** from `WorldImpulse`, **bullet whizzes and supersonic cracks**
  from `ProjectileTrace` passing near the listener, **whooshes** from melee, dash and spell
  `AttackEvent`s.
- **Fire**: burning ground (`HeatField` cells hot enough for flames) is gathered into
  burning areas, and each plays a roar with crackle, pops and snaps that grows with the area
  in flames (a campfire, a burning house, a forest front); burning `Combustible`s (trees,
  buildings) play on their own. The loudest few play, the rest fade out.
- **Water**: small waves lapping at the nearest shores (a slap, the wash running up and
  draining back, the gurgle of the backwash; bigger and slower in wind) and streams babbling,
  growing into a roar in rapids, found by sampling the water and ground around the listener.
  A source that keeps sending `WaterSplash`es (a waterfall's foot, a speedboat's bow) becomes
  one sustained roar that follows how much water comes down, instead of a drip loop.
- **Weather**: rain from `WeatherState` (louder with intensity; ticks on soft ground, taps
  on hard, plinks in puddles, pings on metal; drums on the roof when you're under cover),
  wind (the air moving past your ears, so running into it is louder), and thunder from each
  `LightningStrike` that rolls along the bolt: each stretch of the channel sends its own
  shock wave, so the nearest part cracks first and the rest rumbles in over seconds from up
  and along the bolt, broadside stretches loudest, distant strikes late and dull. (Storms
  with no strikes reported make up their own bolts.)
- **Walls**: sounds behind walls and terrain are quieter and duller (physics raycasts from
  the listener toward each sound, over and beside it too, so a sound round a corner is only
  partly muffled; denser materials muffle more).
- **Space**: distance rolloff, equal-power panning, doppler, air absorption, a little
  shading from behind and below, and sound that starts late by distance / the speed of
  sound. `ReverbZone` boxes (caves, halls, the trail's underpass) add room echo. The air
  comes from `Env::air` at the listener, per planet: sound travels faster in warm air and
  slower in cold, thin air high up makes everything quiet, and in space there is no sound at
  all.
- **Vehicles**: engines and tyres for every vehicle with `EngineState` / `WheelContacts`
  (written by the vehicles module). A piston engine fires pulse by pulse through its
  exhaust's resonances, so revs, load, burble on the overrun, turbo whine and pops follow the
  engine's state; turbines whine and roar, rockets crackle and thunder for kilometres,
  electric motors sing, thrusters hiss. Tyres roll, crunch on gravel, pack snow, squelch in
  mud, hiss on wet roads and squeal when they slide on dry hard ground, reading each wheel's
  ground the way footsteps do (material plus wetness, puddles, snow and frost).
- **Nature**: birds sing from perches 3 to 12 m up, 14 to 65 m around you, nine species
  (warbler, thrush, chickadee, finch, dove, crow, woodpecker, owl, cuckoo), each with its own
  song that a bird keeps from one song to the next. The sun and the hour decide how many: a
  full dawn chorus from first light to a couple of hours after sunrise, fewer birds by day, a
  smaller chorus at dusk, owls only in the dark. Rain, strong wind, snow and cold hush them,
  and towns and bare rock have fewer than woods and meadows (from the ground around you).
  Crickets chirp on warm evenings and nights at the rate the temperature sets (Dolbear's law;
  silent below 8 °C), and cicadas buzz on hot days.
- **Slow motion**: world sounds slow down and drop in pitch with `TimeDilation`, and the
  world gets darker and roomier. Anything with a `LocalTimeScale` that ignores dilation (a
  time-dilated hero) keeps its own pitch.

## Using it

`engine_audio::AudioPlugin` (in `DefaultPlugins` of the `engine` crate) does everything
above with no setup: the active camera listens unless an entity has an `AudioListener`.
To play your own sounds:

```rust,ignore
fn bell(mut audio: ResMut<Audio>, bells: Query<&GlobalTransform, With<Bell>>) {
    for t in &bells {
        let fs = audio.sample_rate();
        audio.play(ToneSound::new(fs, 660.0, 1.0, 0.3), PlayOptions::at(t.translation()));
    }
}
```

Implement `Sound` (fill a buffer, report how much you wrote) for new sounds. `AudioCue`
messages report every sound the module starts (name, place, loudness) for debug views and AI
agents.

Tunable at runtime (JSON, console, scripts, AI agents):

| Name | What |
|---|---|
| `AudioSettings` | output, master volume, speed of sound, doppler, air absorption, how much slow motion lowers pitch, silence in space, footstep range, recorded footsteps on/off, voice limit, weather, fires, water, vehicles, nature and occlusion on/off |
| `AudioMix` | volume per bus (world, ambience, music, ui, voice) and mute |
| `FootstepSurfaces` | how each surface sounds underfoot: 14 layer weights, brightness, pitch, gain |
| `SoundRings` | coloured rings on the ground for each sound near a footstep trail (or everywhere) |
| `AudioListener`, `ReverbZone` | components |
| `audio.status`, `audio.play`, `audio.surfaces`, `audio.record`, `audio.stop_all` | API functions |

### Output and recording

With a window, sound goes to the sound card on its own thread (cpal). Headless (`--headless`,
tests, the screenshot tour), the mix renders offline in step with the frame clock, so a
recording made at any frame rate plays back in real time and in sync with the frames.
Record with `audio.record`, `Audio::start_recording`, or by setting
`ENGINE_RECORD_AUDIO=out.wav` (and optionally `ENGINE_RECORD_AUDIO_SECS`).

On Windows and macOS device output is always built. On Linux build with
`--features engine_audio/alsa` (needs `libasound2-dev`); without it Linux renders offline only,
which keeps CI free of system audio libraries.

### Why not kira

The plan said kira. The mixer here is our own (about 900 lines): every voice needs per-sample
control of pitch for slow motion and doppler, a propagation delay, a medium density for
altitude and vacuum, and the whole mix has to run offline, sample-accurately, under the
engine's manual clock for recordings and tests. Doing that through kira's clock and
tween system cost more than writing the mixer.

## Contracts

Reads (all from `engine_world`): `GroundContact`, `AnimationParams` (speed), `ContactEvent`, `WaterSplash`,
`WorldImpulse`, `ProjectileTrace`, `AttackEvent`, `LightningStrike`, `Collider::surface`,
`SurfaceMaterials`, `SurfaceLayers` (wetness, puddles, snow, scorch), `HeatField` and
`Combustible` (fires), `WaterQuery` (shores and currents), `EngineState` and
`WheelContacts` (vehicles), `WeatherState`, wind, temperature, air, water, ground and
gravity bodies through `Env`, `PhysicsQueries::raycast` (roof over the listener,
walls between the listener and each sound),
`Time` / `TimeDilation` / `LocalTimeScale`, `engine_render::Camera`, and the fetched
recordings in `<asset root>/fetched/audio/footsteps/<surface>/` (`AssetServer::root`, or
`assets/` without one; licenses and credits in `assets/fetched/THIRD_PARTY_NOTICES.md`).

Follows `WorldShift`: the ears, every playing sound and the trail's walkers move into the
new frame, so a floating-origin move is silent.

Writes: `AudioCue` (its own message, moved by world shifts like the shared ones), and the `gravel` surface material when the trail demo
spawns (if nobody defined it). Every trail walker carries a `BodyRequest` (the hiker and runner a
scanned person, the horse and dog the animation module's animals); once a `SkeletonPose`
answers, that walker's stand-in body hides and stops writing `GroundContact`s, since the
animated feet write their own. A kind the module doesn't know keeps its stand-in.

## Test cases

`demo::spawn_demo(commands, origin)` builds the **footstep trail** (about 54 × 22 m): a loop
with one stretch per ground (grass, mud, snow, ice, gravel, a wooden boardwalk, a metal grate,
a concrete underpass that echoes, sand, fallen leaves, puddles), walked by a hiker, a runner
who ignores slow motion, a dog and a horse. Each footstep is a `GroundContact` like a real
character's and shows as a ring in its ground's colour.

- The showcase films it in the valley: `showcase --headless --film footsteps --follow "Trail hiker"`
  (the walkers are named `Trail hiker`, `Trail runner`, `Trail dog` and `Trail horse`).
- `cargo run -p engine_audio --release --example trail_recording -- out/` films the trail
  following the hiker: every ground in turn, the runner overtaking in slow motion, the
  underpass echo and rain soaking the ground at the end. Writes `trail.mp4`, `trail.wav`
  and the frames.
- `cargo run -p engine_audio --release --example footstep_lab -- out/` renders four steps
  on every surface, fires of three sizes, lake shores, a brook, rapids, engines, a tyre
  skidding, thunder at three distances, every bird's song, and crickets and cicadas to WAV
  files
  (`--layers` prints each footstep layer's level).
- `cargo run -p engine_audio --release --example footstep_ab -- out/` renders the same six
  steps on every trail ground synthesized and recorded (`<ground>_ab.wav`), plus light,
  heavy, running, rain-soaked and puddle steps with the recordings (`<ground>_response.wav`);
  `--takes` writes and lists every take, with its noise floor, tail and length in `takes.csv`.
- `tests/plugin.rs`: contacts are heard, out-of-earshot steps are skipped, space is silent,
  the trail walks every surface, walls muffle what's behind them, burning ground and burning
  things are heard, shores lap and brooks babble, sounds stay put when the world shifts,
  lightning rolls thunder late, a waterfall roars instead of dripping, engines rev and tyres
  skid, sound travels at the speed the air allows, and birds sing at dawn, hush in rain and
  give way to owls at night, and recorded takes play where a surface has some (the
  synthesis everywhere else). Unit tests in `samples.rs` cut fake recordings into takes and
  check the cutting and picking rules (takes end with the step, noisy steps and thin surfaces
  left out, no repeats, walk or run, snow and puddles, the fallbacks); none
  needs the fetched files. `tests/screenshot.rs` renders the trail headless.

## Done when

- Footsteps change from grass to mud to snow; rain gets louder with intensity; slow motion
  lowers the pitch. ✓ (all three are in the trail recording and the tests)

Next: music layers; recorded takes for paws and hooves (still synthesized: boot takes
re-pitched for a dog's and a horse's eight feet buried the trail's gaps in noise), cleaner
recordings of mud, ice and puddles (what there is has too much hiss or background), and
recordings for more grounds (dirt, fabric).
