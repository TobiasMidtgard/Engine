# engine_audio — Audio

**Owner:** Audio and footsteps. Background research: `game-engine-research/sound.md` in the
project files.

Spatial sound for the engine. Every sound is synthesized while it plays (no sample files
yet), so footsteps never repeat exactly and follow the ground's state: the same grass sounds
different wet, frozen, snowed on or burnt.

## What you hear

- **Footsteps** from every `GroundContact` (feet, paws, hooves, bodies falling). 18 grounds
  have their own sound: grass, mud, snow, ice, gravel, sand, dirt, rock, concrete, wood,
  metal, glass, water, leaves, fabric, flesh, ash and a default; any other
  `SurfaceMaterial` gets a guess from its density and deformation. Heavier and faster steps
  are louder and sharper. The ground's state (`SurfaceLayers`) changes them: wet ground
  squelches, puddles splash, snow cover crunches over whatever is under it, burnt ground
  crackles, mud freezes hard below 0 °C, and wading drags through water.
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
- **Weather**: rain from `WeatherState` (louder with intensity; ticks on soft ground, taps
  on hard, plinks in puddles, pings on metal; drums on the roof when you're under cover),
  wind (the air moving past your ears, so running into it is louder), and thunder during
  storms that rolls along the lightning bolt: each stretch of the channel sends its own
  shock wave, so the nearest part cracks first and the rest rumbles in over seconds from up
  and along the bolt, broadside stretches loudest, distant strikes late and dull.
- **Walls**: sounds behind walls and terrain are quieter and duller (physics raycasts from
  the listener toward each sound, over and beside it too, so a sound round a corner is only
  partly muffled; denser materials muffle more).
- **Space**: distance rolloff, equal-power panning, doppler, air absorption, a little
  shading from behind and below, and sound that starts late by distance / 343 m/s.
  `ReverbZone` boxes (caves, halls, the trail's underpass) add room echo. Air thins with
  altitude: high up it gets quiet, and in space there is no sound at all.
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
| `AudioSettings` | output, master volume, speed of sound, doppler, air absorption, how much slow motion lowers pitch, silence in space, footstep range, voice limit, weather, fires, water and occlusion on/off |
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

Reads (all from `engine_world`): `GroundContact`, `ContactEvent`, `WaterSplash`,
`WorldImpulse`, `ProjectileTrace`, `AttackEvent`, `Collider::surface`, `SurfaceMaterials`,
`SurfaceLayers` (wetness, puddles, snow, scorch), `HeatField` and `Combustible` (fires),
`WaterQuery` (shores and currents),
`WeatherState`, wind, temperature, water,
ground and gravity bodies through `Env`, `PhysicsQueries::raycast` (roof over the listener,
walls between the listener and each sound),
`Time` / `TimeDilation` / `LocalTimeScale`, `engine_render::Camera`.

Follows `WorldShift`: the ears, every playing sound and the trail's walkers move into the
new frame, so a floating-origin move is silent.

Writes: `AudioCue` (its own message), and the `gravel` surface material when the trail demo
spawns (if nobody defined it).

## Test cases

`demo::spawn_demo(commands, origin)` builds the **footstep trail** (about 54 × 22 m): a loop
with one stretch per ground (grass, mud, snow, ice, gravel, a wooden boardwalk, a metal grate,
a concrete underpass that echoes, sand, fallen leaves, puddles), walked by a hiker, a runner
who ignores slow motion, a dog and a horse. Each footstep is a `GroundContact` like a real
character's and shows as a ring in its ground's colour.

- `cargo run -p engine_audio --release --example trail_recording -- out/` films the trail
  following the hiker: every ground in turn, the runner overtaking in slow motion, the
  underpass echo and rain soaking the ground at the end. Writes `trail.mp4`, `trail.wav`
  and the frames.
- `cargo run -p engine_audio --release --example footstep_lab -- out/` renders four steps
  on every surface, fires of three sizes, lake shores, a brook, rapids, engines, a tyre
  skidding and thunder at three distances to WAV files
  (`--layers` prints each footstep layer's level).
- `tests/plugin.rs`: contacts are heard, out-of-earshot steps are skipped, space is silent,
  the trail walks every surface, walls muffle what's behind them, burning ground and burning
  things are heard, shores lap and brooks babble. `tests/screenshot.rs` renders the trail headless.

## Done when

- Footsteps change from grass to mud to snow; rain gets louder with intensity; slow motion
  lowers the pitch. ✓ (all three are in the trail recording and the tests)

Engines (piston engines firing pulse by pulse through their exhaust, turbines, rockets,
electric motors, thrusters) and tyres (rolling, crunching, squelching, hissing, squealing)
are synthesized in `vehicle.rs` and render in `footstep_lab`; they play in the world once
the vehicles module publishes engine and wheel state through `engine_world` (asked of the
lead).

Next: vehicles in the world (see above), thunder from the
weather's own lightning strikes (`LightningStrike`, waiting on the lead's contract), music
layers, and recorded samples alongside the synthesis.
