# engine_npc — NPC AI

**Owner:** Smart NPC AI. **Status:** AI core in (perception, memory, tactics, navigation,
squads, melee and ranged combat, learning, explosives). Wildlife, stand-in bodies, the showcase
arena and the `npc.*` API land next.

Enemies that feel smarter than other games: they notice you gradually, remember where you went,
use cover and high ground, flank and suppress as a squad, take turns in melee instead of
waiting in a ring, block, parry and dodge telegraphed attacks, feint, punish your recovery,
learn your habits, and shoot the explosive barrel next to you. Every part of that is tunable
per NPC through skills.

## Using it

```rust,ignore
app.add_plugins(NpcPlugin);
commands.spawn((
    Transform::from_xyz(0.0, 0.0, -20.0),
    Npc::default(),                 // adds everything else with defaults
    NpcSkills::veteran(),           // or rookie / soldier / elite / master / at_level(0.7)
    NpcWeapon::rifle(),             // sword, sword_and_shield, rifle, bow, claws
    Faction::ENEMY,
    Squad(1),                       // squad mates share sightings and coordinate
));
// Put a Faction on the player so NPCs know whose side it is on.
commands.entity(player).insert(Faction::PLAYER);
```

Static `Collider`s (walls, crates, platforms) become the NPCs' tactical world: cover points,
line of sight, walkable tops and the navigation grid are built from them and rebuilt when they
change. When a physics backend is installed, sight and shot checks also use `PhysicsQueries`.

## Skills (`NpcSkills`, all 0..1 except reaction time)

| Skill | What it changes |
| --- | --- |
| `reaction_time` (s) | NPCs act on what they perceived this long ago. Blocks, dodges, aim and decisions all use the delayed picture, so a 0.15 s master blocks a 0.3 s telegraph and a 0.6 s rookie doesn't. Measured on the NPC's own clock, so slowing an NPC (or the world) with `LocalTimeScale` / `TimeDilation` slows its reactions too: with high perception the player sees enemies react late. |
| `accuracy` | Shot spread and lead on moving targets. |
| `logic` | How many positions it weighs, whether it uses cover, high ground, flanking and explosives, and how far ahead it plans. |
| `aggression` | Pushes forward and attacks more often vs. holds and waits for openings. |
| `courage` | When it retreats (hurt, outnumbered, under fire). |
| `defense` | Chance and quality of block, parry and dodge. |
| `awareness` | Detection speed, peripheral vision, hearing. |
| `teamwork` | Callouts, waiting its turn, covering fire, flanking for mates. |
| `learning` | How fast it learns your attack rhythm, favorite dodge side and habits. |
| `agility` | Movement and turn speed, dash. |

`Perception` holds the senses (sight range, field of view, eye height, detection and memory
seconds). `NpcWeapon` holds damage, reach or range, windup (the telegraph), recovery (the
punish window), cooldown, spread and shield.

## How it thinks

1. **Prepare**: rebuild the tactical world when colliders change; advance each NPC's clock in
   its own time scale; gather everything that can be seen.
2. **Perceive**: sight cones with occlusion, hearing (`NoiseEvent`, gunfire, impacts), being
   hit (`DamageEvent`, `ProjectileTrace` near misses). Awareness grows over
   `detection_secs`, faster when close, moving or loud. Memory keeps last-known positions with
   timestamped snapshots that the reaction delay reads from.
3. **Decide**: squads share sightings, hand out roles (assault, suppress, flank, hold) and melee
   attack tokens. Each NPC scores its options (patrol, investigate, search, engage, take cover,
   flank, high ground, use explosive, retreat) with utility curves shaped by its skills, then
   picks a spot by scoring candidates for cover, line of fire, distance band, height, flanking
   angle, danger (fire, explosives) and path length on the nav grid. Combat decides strikes,
   heavy guard breakers, feints, dash attacks, blocks, parries and dodges against the attacker's
   telegraph.
4. **Act**: the built-in fallback mover (only when no movement module has claimed
   `LocomotionBackend`), hit resolution, damage, explosive props, revive.

## Contracts

Reads: `Collider` / `RigidBody` (tactical world), `PhysicsQueries` when present, `Ground`,
`Water`, `HeatField` (avoid fire), `NoiseEvent`, `DamageEvent`, `AttackEvent` (telegraphs to
defend against), `ProjectileTrace` (near misses), `LocomotionState` of targets, `TerrainChanged`,
`LocalTimeScale` / `TimeDilation`, `LocomotionBackend`.

Writes: `MoveIntent` (the same input a player produces), `LocomotionState` and
`AnimationParams` (fallback mover only), `AttackEvent` (its own telegraphs), `DamageEvent`,
`ProjectileTrace` (its shots), `NoiseEvent` (shouts and gunfire), `GroundContact` (footsteps),
`WorldImpulse`, `IgniteRequest` and `SmokeEmission` (explosions).

The crate depends only on core crates (`engine_core`, `engine_world`, `engine_render`).

## Tests

- `cargo test -p engine_npc` runs the unit tests (nav, tactics, perception, combat math) and the
  behavior tests in `tests/behavior.rs`, which run whole NPCs headless and check that they see
  what's in front and not through walls, that reaction time decides who blocks, that slow motion
  slows reactions, that shooters take cover when shot at, that melee fighters close in and hit,
  that a master beats a veteran in a duel, that squad mates share sightings, that melee squads
  take turns, that shooters detonate explosives next to enemies, and that a ranged squad sends a
  flanker.

## Done when

- Enemies that feel smarter than other games: they take cover, flank, punish mistakes, and block.
- Dynamic fauna roams, herds, drinks, rests and flees from threats, fire and explosions.
- The showcase arena shows all of it with readable stand-in bodies and an intent overlay.
