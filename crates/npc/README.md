# engine_npc — NPC AI

**Owner:** Smart NPC AI. **Status:** AI core (perception, memory, tactics, navigation, squads,
melee and ranged combat, learning, explosives), wildlife, sculpted fighters and animals, the
stone showcase arena, the `npc.*` API and smoke-aware sight are in. Next: lifelike faces,
vehicles.

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

## Wildlife

```rust,ignore
commands.spawn((Transform::from_xyz(5.0, 0.0, 3.0), Animal::new(Species::Deer), Herd(1), StandIn::default()));
```

Deer and rabbits graze, wander with their herd, drink at the nearest water's edge, bed down at
night and bolt from anything inside their flight distance. That distance grows when the
threat runs or attacks and shrinks a lot when it sneaks, so a crouching player gets close.
Deer smell what's upwind. Panic spreads through a herd a moment after the first one bolts, and
alert deer make the rest look up. Wolf packs roam, get hungry, track prey by scent, stalk low
while spreading out around it, then charge, cut it off and bite; they feed on the kill and
back away from people. Bird flocks peck on the ground and lift off together. Gunfire,
explosions, howls and fire scare everything. `WildlifeSettings` tunes respawn, thirst, hunger,
skittishness and howling.

## Bodies and art

`StandIn` gives an NPC or animal a body posed every step from the AI state, so behavior reads
on screen: gait, crouching in cover, the raised weapon of an attack's telegraph (glowing for a
heavy guard breaker), blocks, parry flashes (the blade glints white, the shield lights in a
pale tint of the team color), dodges, staggers, hit flashes (struck plates go a pale tint of
the team color), falling when downed, muzzle flashes, a rank crest in the skill color and an
alert light. Fighters plant their feet (a planted foot keeps its spot while the body moves over
it; a test fails if a grounded foot slides), stride longer as they speed up, shuffle and
side-step on guard, coil onto the rear foot in a telegraph and drive onto the front foot in the
swing, brace into blocks, hop wide in a dodge and stumble back when staggered. Guards and
parries meet the line of the incoming blow (overhead for a heavy cut, low across for a lunge,
left or right), a dash pushes off the rear foot, a feinted blow checks and returns to guard,
hits throw the head, trunk or legs by where the cut lands (heavy cuts high, cuts on the run
mid, other cuts mid or low), and a downed fighter falls from whatever pose it was in: knees
give, hips, back and head land, limbs settle and the weapon drops beside it. A heavy cut is
met in a roof guard over the helmet, a cut to the head wrenches it away with a half step back,
carbines kick and settle and change magazines between shots, archers draw to the mouth, hold,
loose and nock the next arrow from the quiver, fighters turn on the spot by stepping round, and
crests, tabards and pouches swing on after a stop or a blow. Animals plant their feet too (a
test fails if a hoof or paw on the ground slides) in each species' footfalls: deer walk in
lateral sequence, trot and gallop (rotary) and bound away when they bolt from a stand, wolves
walk, trot, lope and gallop, rabbits hop and half-bound with the hind feet landing ahead of the
fore feet. Strides lengthen and quicken with speed; the spine flexes and extends through the
gallop, the body flies an arc when no foot is down, sits back setting off and pitches forward
stopping, leans into turns and breathes harder after a run, and heads steady against the bob.
Ears swivel toward noises and threats and flick, tails swish (a deer's flags up when alarmed),
grazing deer tear the grass and look up to chew and watch, an alert animal throws its head up
toward what it heard and freezes, and states blend into each other. Starlings beat their
wings (extended downstroke, folded upstroke), bound on folded wings between bursts, glide,
bank into turns, push off with their legs to take off and flare to land. With
`NpcSettings::show_intent`, rings and lines on the ground show where each NPC is going and why
(blue cover, orange flank, purple high ground, grey fall back, yellow search).

All art is generated in code (`src/art`), since no third-party art may go in the repository:

- **Fighters** (`art/fighter.rs`): armored arena combatants in quilted linen gambesons, wool
  hose and leather, with painted plates (a cuirass that wraps the flanks, pauldrons, helmet),
  knee-length tabards front and back and team-dyed brassards, so squads tell apart at 30 m.
  Paint wears with skill: rookies are clean, veterans and masters chipped, dented and dirty.
  Open helmets with a brow light and a rank crest sit over mail coifs; faces come in four
  shapes, six skin tones and four eye colors, and gambeson and hose shades vary per fighter.
  Weapons are a longsword, an arming sword with a heater shield, a carbine or a recurve bow. A
  skeleton with two-bone IK for arms and legs poses them for every combat state. Each team
  shares one mesh per piece.
- **Wildlife** (`art/animals.rs`): sculpted red deer (stags with antlers and a neck mane),
  grey wolves and European rabbits, with fur textures that follow the coat, baked ambient
  occlusion, IK legs and planted feet, and starlings with iridescent spotted plumage and
  feathered wings that fold, spread and beat.
- **Arena** (`art/arena.rs`): a sandstone amphitheater with arched stands, gates with
  portcullises, fluted columns, banners, braziers with flickering fire and light, rubble,
  timber barricades, crates and drums, all with baked PBR textures. The floor (`art/arena/
  grounds.rs`) is lived in: raked sand in overlapping lanes, trampled tracks with footprints
  between the gates, ring, lane and cover, cart ruts and drag marks at the gates, dust drifts
  against the walls and columns, straw, broken arrows and splinters, and stone troughs, water
  casks and weapon racks along the walls with soaked sand where water was spilled. The art
  dresses the gameplay entities; colliders never change.

Everything is built once per world and shared. Without a renderer (headless tests) the art
falls back to primitive shapes so tests stay fast.

## API (`npc.*`, JSON)

AI agents and tools reach NPCs through the engine's API registry (and so through the AI bridge):

| Function | Does |
| --- | --- |
| `npc.list` | NPCs and animals with position, action or state, target, health; filter by `near`/`radius`/`kind` |
| `npc.spawn` | a fighter: `position`, `skills` (preset, level 0..1 or fields), `weapon`, `faction`, `squad`, `name` |
| `npc.spawn_animal` | `species` deer, rabbit, wolf or bird, `herd`, `range`, `male` |
| `npc.set_skill` | merge skill fields (clamped) or set a preset or level |
| `npc.inspect` | why an NPC does what it does: utility scores, chosen spot and best candidates, what it knows, combat state and stats; for animals state, danger and needs |
| `npc.order` | `hold`, `move`, `attack` or `clear` for one NPC or a list |
| `npc.alert` | a noise at a point: NPCs investigate, animals startle |
| `npc.despawn` | remove an NPC or animal |

Entities are addressed by `id` or `name`.

## Showcase

`demo::spawn_demo(commands, origin)` builds the whole combat arena (`demo::ARENA_RADIUS`, 48 m):
a sand floor ringed by stepped stone stands with red and blue banners, pillars, braziers, a ramp
and a wall-run wall. Two five-fighter squads of mixed skill fight across low walls, crates,
explosive barrels and a raised platform; a master and a veteran duel in a ring; and a sparring
lane on the east side puts five sword-and-shield fighters, rookie to master, in front of posts
that swing at them, so you can watch skill decide who gets hit, who blocks and who parries.
Downed fighters get up after six seconds and barrels come back, so the fight keeps going. A
camera at (0, 16, −31) from the origin looking at (0, 0, 4) frames the fight. `demo::spawn_wildlife(commands, center, radius)` fills a meadow with a
deer herd, rabbits, two bird flocks and a wolf pack.

## Training

`SparringPost { target, .. }` is a post with a swinging arm that attacks one NPC on an uneven
beat with a clear telegraph, through the same `AttackEvent` a player sends. It counts blocks,
parries, dodges and hits, so it doubles as a test rig for skills.

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
   `detection_secs`, faster when close, moving or loud, and slower through smoke and fog
   (`Obscurance`) in step with the light that gets through; past 8 % nothing is seen at all.
   Memory keeps last-known positions with timestamped snapshots that the reaction delay reads
   from.
3. **Decide**: squads share sightings, hand out roles (assault, suppress, flank, hold) and melee
   attack tokens. Each NPC scores its options (patrol, investigate, search, engage, take cover,
   flank, high ground, use explosive, retreat) with utility curves shaped by its skills, then
   picks a spot by scoring candidates for cover, line of fire, distance band, height, flanking
   angle, danger (fire, explosives) and path length on the nav grid. Thick smoke counts as
   concealment when there is no solid cover (it hides but doesn't stop bullets, so solid cover
   wins), and shots into smoke spread wider. Combat decides strikes,
   heavy guard breakers, feints, dash attacks, blocks, parries and dodges against the attacker's
   telegraph. Elites and masters fight on the counter: they never swing into an attack they
   can parry, they let it come, deflect it and punish the opening.
4. **Act**: the built-in fallback mover, hit resolution, damage, explosive props, revive. Once
   a movement module has claimed `LocomotionBackend`, the fallback mover leaves bodies with a
   `CharacterController` to it: give an NPC the movement module's character components (in
   game code) and it wall-runs and slides like the player; NPCs without them keep the
   built-in mover.

When the world is re-anchored (a floating-origin move or a new reference frame, `WorldShift`),
everything NPCs and animals keep in world space moves with it in `WorldShiftSet::Apply`: homes,
orders, destinations and candidate spots, paths, remembered positions, velocities and
snapshots, disturbances, aim points, dashes, threats, noises heard, water and landing spots, and
the tactical world. The navigation grid rebuilds its chunks where paths are next asked for, and
a turned world also rebuilds the tactical world from the colliders.

## Contracts

Reads: `Collider` / `RigidBody` (tactical world), `PhysicsQueries` when present, `Ground`,
`Water`, `HeatField` (avoid fire), `Obscurance` (smoke, fog and dust), `NoiseEvent`, `DamageEvent`, `AttackEvent` (telegraphs to
defend against), `ProjectileTrace` (near misses), `LocomotionState` of targets, `TerrainChanged`,
`LocalTimeScale` / `TimeDilation`, `LocomotionBackend`, `WorldShift`.

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
  that a master beats a veteran in most of six duels and takes far fewer hits, that squad mates
  share sightings, that melee squads take turns, that shooters detonate explosives next to
  enemies, and that a ranged squad sends a flanker.
- `tests/wildlife.rs`: deer bolt from a runner but let a sneaker much closer, panic spreads
  through a herd, a wolf pack stalks, charges, kills and feeds, birds take off together and
  land again.
- `tests/demo.rs`: the arena fight keeps going for a minute (shots, attacks, blocks, cover,
  downed and revived) inside the arena; the meadow lives for two minutes (grazing, wandering,
  hunting, fleeing) without animals leaving.
- `tests/training.rs`: the same post swings at a rookie, soldier, veteran, elite and master;
  the share of swings defended climbs with skill (0 % to about 100 %) and the master parries.
- `tests/smoke.rs`: thick smoke hides a player until it clears, thin smoke slows spotting,
  shots into smoke miss more, and a shooter with no solid cover hides in a smoke bank.
- `tests/api.rs`: spawn, tune, order, alert, inspect, list and despawn through `npc.*`, and
  bad arguments come back as errors.
- `tests/world_shift.rs`: a far floating-origin move, and two shifts in one frame that turn the
  world, mid-fight and while deer flee: memories, destinations, paths, aim, homes, threats and
  the tactical world move exactly with the world, the rifleman keeps looking and shooting at
  the player, the deer keep running from what scared them and stay around their home, and
  paths go around a wall where it moved to, not where it was.
- `tests/art_fighters.rs`, `tests/art_animals.rs`, `tests/art_arena.rs`: close-up renders of
  the fighters, wildlife and arena in chosen poses and lights, flipbook strips of the wildlife
  on the move (gaits, starts and stops, an alarm, a bend, a stalk, a starling's flight;
  `NPC_STRIPS=deer-walk,...` shoots only some), plus triangle and build-time budgets and a
  check that the arena art sits on every collider.
- `tests/screenshots.rs`: renders the arena, its stands, a cover fight, the sparring lane, the
  duel and the meadow on a GPU or lavapipe and checks the fighters and deer show up.
  `NPC_SCREENSHOT_DIR=dir` keeps the PNGs.

## Done when

- Enemies that feel smarter than other games: they take cover, flank, punish mistakes, and block.
- Dynamic fauna roams, herds, drinks, rests and flees from threats, fire and explosions.
- The showcase arena shows all of it with readable bodies and an intent overlay.
