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
change. A grid cell is walkable only with no obstacle side within the agent radius (0.4 m) that
rises more than a step and starts below head height, so paths keep clear of wall ends and pinches
instead of running into them (a tilted ramp counts from its lowest corner, so the way under its
high end is not taken); a goal just inside that clearance (a cover spot by a wall) is still
reached with a last straight step, and a body standing inside it starts from the nearest free
cell at its own level (not the top of the barrel beside it). When a physics backend is installed, sight and shot checks
also use `PhysicsQueries`.

## Wildlife

```rust,ignore
commands.spawn((Transform::from_xyz(5.0, 0.0, 3.0), Animal::new(Species::Deer), Herd(1), StandIn::default()));
```

Deer and rabbits graze, wander with their herd, drink at the nearest water's edge, bed down
and bolt from anything inside their flight distance. Deer and boar keep the world's hours
(`DayPart`, from `WeatherState`'s clock and sun, as the day/night cycle writes them): they
move out at dawn and dusk, up from their beds one by one and down to water to drink, graze
through the morning and afternoon, and bed down at midday (`MIDDAY_H`) and in the dark, each
herd lying up close together (`BED_SPREAD`). They drink at a pond's shore or a stream's
bank alike (anything `Water` reports): each goes to its own place along the bank, stands
square on to the water a muzzle's reach back from the edge, and drinks, head down (a boar
laps rather than roots). Bedded deer chew the cud, heads up, and now and then doze with the
chin laid on the ground; rabbits bed down in the dark; wolves hunt through it. That distance grows when the
threat runs or attacks and shrinks a lot when it sneaks, so a crouching player gets close.
Deer smell what's upwind. Panic spreads through a herd a moment after the first one bolts, and
alert deer make the rest look up. Wild boar live in sounders (sows with their striped
piglets, `Animal::young`) or alone (tuskers, `Animal::male`), root for food with the snout,
shuffling on as they go. They run from threats like the rest, but a
boar turns and charges when it is cornered (its flight finds no way out, or the threat is on
top of it) or wounded (it goes for whoever hurt it, not for whatever it happened to be
watching; when it can't find who, it stands and scents for them, bristling, rather than
charge at nothing), and a sow charges anyone who comes close to her piglets while they scatter; the
charge ends in a slash of the tusks (damage), then the boar breaks off and runs for a moment
before it may turn again. Fire always means flight: a boar never turns to fight in the
flames. Wolf packs roam, get hungry, track prey by scent and hunt together, each wolf in a
part by its place in the pack (`AnimalMind::role`, a `PackRole`): the tester creeps straight
at the herd, singles out the weakest (wounded, young or straggling; it picks over the herd
again as its rush sets it running) and drives it; two flankers swing wide to its sides and
run out level with it to cut its escape line, holding its side until it tires or a wolf is
on it, then cut in; a fourth wolf circles wide round the herd and lies in wait where it
will run, still and low once the pack sets off (a deer fleeing the pack doesn't see it
until it is almost on it), springing when it comes within `AMBUSH_RANGE` (or the pack's
charge misses, or it has waited a minute); once the quarry is past, it drops back onto its
trail and follows on at a run rather than cut in from the side; any more drive with the
tester. While they stalk, the wolves keep 10 m
clear of the herd and crouch within 30 m of it, and the flankers loop round behind it to
their posts out wide, about level with the quarry (`FLANK_POST`), rather than cross in
front. The tester rushes once the pack is
in place. A pack gives up a chase
`PACK_CHASE_SECS` after its first wolf set off, or sooner, once its test is over, from a fit
(unhurt, grown) quarry that is `ESCAPE_GAP` ahead of its nearest wolf and still pulling away
(a deer at full sprint outruns a wolf), all of it together, on the wolves' own clocks, and
rests `PACK_REST_SECS`, hungry or not. They feed on the kill and back away from
people. Deer raise the alarm: the first to bolt snorts (a noise other animals hear, though
not its own herd) and flags its tail; each of the others turns its head to where the alarm
came from, then bolts a reaction time later (0.3 to 0.55 s), and the herd runs off bunched
(the ones out front ease off for the rest). The deer the pack is running down is cut off
from its herd: it no longer runs with the others or is drawn back to them (it only keeps
clear of them), so it runs its own line from the wolves (a unit test in `wildlife.rs`
checks there is no pull to the herd's middle and no running with it). A deer runs from all the wolves it sees at
once, the nearest most, so one with a wolf on either side runs on between them; with a wolf
close at its heels (`ZIGZAG_RANGE`) it cuts from side to side at sharp angles; the one that
raised the alarm at a predator it saw stots (pronks) for its first bounds (`STOT_SECS`;
not at a bang or fire, when wounded, or with a wolf close); a wounded animal runs lame. A blow is felt at once (the animal reacts the same tick). Wolves prefer deer, take a boar only when nothing easier is about (a
piglet sooner, a tusker last of all) and give room to a boar charging at them (not to one
charging past them at something else). Bird flocks peck on the
ground and lift off together. Gunfire, explosions, howls and fire scare everything. A
gunshot (a bullet's `ProjectileTrace` faster than sound; the bang it makes is not counted
twice) carries `GUNSHOT_REACH` times an animal's hearing: birds in earshot burst up, and
deer, boar, rabbits and wolves look up toward it, then move off from it at a brisk walk
(`WARY_DIST`), carrying on if more shots follow; one closer than `GUNSHOT_BOLT` sends them
running. An explosion (a `WorldImpulse` with energy or heat behind it, or a strong one, such
as a barrel going up or a scripted blast) carries `BLAST_REACH` times their hearing, and
everything that hears it bolts; an arrow, or a light thump, does nothing.
`WildlifeSettings` tunes respawn, thirst, hunger, skittishness and howling.

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
crests, tabards and pouches swing on after a stop or a blow. Fighters look at their foe (eyes
first, then the head), glance at flankers and noises, blink, set the jaw for a heavy blow and
shout as it lands, grimace when hit and breathe hard when tired; badly hurt, they limp on the
struck leg or hunch over a body wound and press a hand to it at ease. At ease the weapon rests
on the shoulder or hangs low and comes to guard when a foe is spotted, and a revived fighter
rolls over, kneels, takes up its weapon and stands into its guard where it lay. A fighter
reviving a mate goes down on both knees beside it, on the side of the arm along the body
(clear of the arm flung out and of what the mate let go of). It sets its sword or carbine down
on the ground; an archer or a shield bearer holds the bow or shield aside. It puts its hands on
the chest and presses, and looks up at the enemy now and then. Then it takes its weapon up and
stands. Shot down on its knees, it falls from there, its weapon left where it set it. Animals
plant their feet too (a
test fails if a hoof or paw on the ground slides) in each species' footfalls: deer walk in
lateral sequence, trot and gallop (rotary) and bound away when they bolt from a stand, wolves
walk, trot, lope and gallop, boars walk, trot and gallop (transverse) on quick, short strides
(the fore feet coming down under the chest, folded high as they swing; piglets scamper,
quicker still and bouncing high),
rabbits hop and half-bound with the hind feet landing ahead of the fore feet. Strides lengthen
and quicken with speed; deer, wolf and boar backs bend at the loin,
arching as the hind feet swing up past the fore feet and hollowing as the legs stretch out
through the gallop, the body flies an arc when no foot is down, sits back setting off and
pitches forward stopping, leans into turns and breathes harder after a run, and heads steady
against the bob. A stotting deer pronks: all four feet push off and land together on legs
held stiff and straight, the back arched, each bound high, the body near level and the head
held high, riding up with each bound, the white rump patch flared out of the coat (quickly
up, slowly down after). Ears swivel toward noises and threats and flick, tails swish (a deer's flags
up when alarmed), grazing deer tear the grass and look up to chew and watch, an alert animal
turns its head toward what it heard before its neck follows and freezes, and states blend
into each other. A hunting wolf stalks low with its head on its prey, rushes from the stalk
crouched, reaches with jaws open as it closes and snaps, grips and shakes on the bite. A
rooting boar shoves its snout into the soil and levers it up, sweeping from side to side with
the jaw working, and lifts its head now and then to scent the air; alarmed, it champs its
jaws; it runs with its tail up, and charges head down to hook its tusks up and sideways
through what it strikes; alarmed or charging, its crest of long, pale-tipped bristles stands
up along its nape and withers. Hurt but not killed, an animal flinches, big enough to read
at 25 m: it jolts and rolls away from the blow, hunched and ducking, its forequarters
swinging away about the hind hips with the front feet stepping out after them, head and
neck swinging hard away (so a blow from the far side reads from this side too), ears
pinned flat and tail clamped, stumbles as a foreleg gives, hitches in its stride and
staggers a step or two before it runs. A killed animal goes down forequarters first (onto its
chest and knees on the run), then over onto its side away from the blow, and settles. Starlings beat their wings (extended
downstroke, folded upstroke), bound on folded wings between bursts, glide, bank into turns,
push off with their legs to take off and flare to land, and tumble out of the air when
killed. With
`NpcSettings::show_intent`, rings and lines on the ground show where each NPC is going and why
(blue cover, orange flank, purple high ground, grey fall back, yellow search, teal revive).

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
  grey wolves, wild boar (front-heavy and wedge-shaped: a deep chest and massive shoulders
  under the hump of the withers, the back sloping to a narrow, low rump, short stout legs, a
  short thick neck and a big wedge head tapering to the flat snout disc; a bristly dark coat
  with a crest of pale-tipped bristles down the nape and over the withers that rises when the
  boar is roused, small eyes and pricked ears, cloven trotters with dewclaws, curved tusks on
  the tuskers, the upper pair in the lip and the lower on the jaw, and a size down,
  pale-striped piglets, striped down to the jaw) and European rabbits, with fur
  textures that follow the coat (a deer's hooves are part of the pastern piece, to keep the
  stag within its part budget, yet dark, smoother horn: their vertices sample a few-texel
  horn swatch baked into a corner of the coat texture), baked ambient occlusion, IK legs and planted feet, deer, wolf
  and boar torsos in two halves that bend at the loin over a closed seam (no crease: the fore
  half's rim lies on the skin with its normals, and the croup swells a hair over the fore
  half's hinge where the back slopes away behind it), and starlings with
  iridescent spotted plumage and feathered wings that fold, spread and beat.
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

## Faces and bodies from a character module

Fighters are ready for the faces and bodies a character module draws (`engine_world::
character`; `visuals/character.rs`):

- **Looks.** On its first step each fighter gets an `AppearanceRequest`: its own `Seed`, its
  squad (with its faction) as the family seed so squad mates look related, a sex picked from
  the seed and an age range by skill (a rookie 18 to 26, a master 34 to 52). An
  `Appearance` already there is kept. No full `Appearance` is put on fighters.
- **Pose and face.** One writer per entity. On fighters without a `BodyRequest` this crate
  writes `SkeletonPose` every step from the stand-in's pose, on the animation module's
  humanoid skeleton (its bone names and parents, shared by every fighter through one `Arc`;
  model space, facing −Z). The joints the stand-in has are put on it exactly: waist
  (`spine`), neck, shoulders, elbows, wrists, hips, knees and ankles. Each is turned like the
  segment it starts, with limbs along −Y, the hinge on +X and the feet flat toes −Z, so
  straight legs read as the rest pose. `hips`, `chest`, `head`, the clavicles, toes and
  ponytail sit at the animation module's offsets. The limbs keep the stand-in's lengths
  (thigh 0.45 m, shin 0.44 m, upper arm 0.30 m, forearm 0.26 m). This crate also writes
  `FaceControls` with ARKit weights:
  - blinks, shut when downed;
  - pain from a blow's flinch and a wound's ache: squint, brows down, nose sneer, mouth
    stretched and down;
  - effort from a heavy blow set and shouted, a sprint, a braced guard, a held draw and a
    revive: squint, brows down, lips pressed;
  - fear when pinned down by fire or badly hurt in a fight: inner brows up, eyes wide;
  - panting: the jaw opening with each breath;
  - gaze yaw and pitch, the eyes' turn in the head.

  Feet still send `GroundContact`s, now each with a `footstep_left` or `footstep_right`
  `AnimationEvent` (bone `foot_l` / `foot_r`).
- **Hiding the stand-in.** While the `AppearanceBodies` resource exists (checked every step),
  a fighter with an `Appearance` or an `AppearanceRequest` has its stand-in body hidden. What
  it holds stays shown: the weapon, the shield, a magazine or arrow in hand, the muzzle
  flash. Without the resource nothing changes.
- **Wear.** On fighters with an `Appearance`, `wear` follows the fight:
  - blood from cuts, stabs and shots (less from blunt blows and burns);
  - bruises from blunt blows and falls;
  - dirt from going down, lying on the ground, kneeling by a mate and dashing.

  A fighter sent home fresh is clean again. `wet` is left to the weather.

## API (`npc.*`, JSON)

AI agents and tools reach NPCs through the engine's API registry (and so through the AI bridge):

| Function | Does |
| --- | --- |
| `npc.list` | NPCs and animals with position, action or state, target, health; filter by `near`/`radius`/`kind` |
| `npc.spawn` | a fighter: `position`, `skills` (preset, level 0..1 or fields), `weapon`, `faction`, `squad`, `name` |
| `npc.spawn_animal` | `species` deer, rabbit, wolf, boar or bird, `herd`, `range`, `male`, `young` |
| `npc.set_skill` | merge skill fields (clamped) or set a preset or level |
| `npc.inspect` | why an NPC does what it does: utility scores, chosen spot and best candidates, what it knows, combat state and stats; for animals state, danger and needs |
| `npc.order` | `hold`, `move`, `attack` or `clear` for one NPC or a list |
| `npc.alert` | a noise at a point: NPCs investigate, animals startle |
| `npc.despawn` | remove an NPC or animal |

Entities are addressed by `id` or `name`.

## Showcase

`demo::spawn_demo(commands, origin)` builds the whole combat arena (`demo::ARENA_RADIUS`, 48 m):
a sand floor ringed by stepped stone stands with red and blue banners, pillars, braziers, a ramp
and a wall-run wall. Two six-fighter squads of mixed skill (a medic in each) fight across low walls, crates,
explosive barrels and a raised platform; a master and a veteran duel in a ring; and a sparring
lane on the east side puts five sword-and-shield fighters, rookie to master, in front of posts
that swing at them, so you can watch skill decide who gets hit, who blocks and who parries.
A squad mate often runs to a downed fighter, kneels and gets it up where it lies. One that
nobody reaches gets up after six seconds, and barrels come back, so the fight keeps going. A
camera at (0, 16, −31) from the origin looking at (0, 0, 4) frames the fight. `demo::spawn_wildlife(commands, center, radius)` fills a meadow with a
deer herd, rabbits, two bird flocks, a sounder of wild boar with piglets and a lone tusker
along the woods edge, and a wolf pack.

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

## Squad revives

A squad mate can revive a downed fighter where it lies. Each mate weighs the run: how far it
is, how much the spot is in the enemy's view, how much fire the mate is under, its own health,
and whether the squad is winning. Sight lines go through smoke, so smoke over the spot helps.
An enemy that looks elsewhere, or that a mate is shooting at, counts for less. Only the best
placed mate of the squad goes. Ranged mates with some teamwork cover it with suppressive fire.

Skill changes the choice. A rookie notices late, then runs straight in. A skilled fighter waits
while the spot is watched or it is under fire, and goes once things calm down. The reviver runs
over and kneels beside the chest, on the side of the arm along the body (the other side if
that one is blocked). Getting down, setting its weapon down and putting its hands on takes
1.2 s (`NpcBrain::REVIVE_KNEEL_SECS`). The revive counts only once the hands are on. After
`ally_revive_secs` of that, the ally gets up where it lies with `ally_revive_health` of its
health. Teamwork 1 makes it a fifth quicker, teamwork 0 a fifth slower. A mate weighs every
downed squad mate in range and goes for the one it most wants to, so an ally in the open does
not keep it from one behind a wall. A kneeling reviver can neither block nor dodge. A reviver
that is hit, staggered or stuck (its way along the path stops getting shorter), or that finds
it too hot, lets go. It gets up off its knees where it is before it moves (0.85 s), and waits a
moment before it tries again. A fighter just revived revives nobody until it is up. One handed
to game code (`Npc::active = false`) gives its revive up, and one stopped in time revives
nobody, so neither holds up the showcase timer. `npc.inspect` shows `kneeling_secs` and
`hands_on_secs`.

### Squad medics

`Npc::role = NpcRole::Medic` makes a fighter its squad's medic (the arena's squads have one
each, with teamwork 1 and a carbine, `NpcWeapon::carbine()`: lighter and shorter-ranged than
the rifle). A medic fights from 3 m behind where it would otherwise stand, notices a downed
mate twice as soon, goes half as far again (`ally_revive_range` × 1.5) and wants the revive
far more, so it is usually the one that goes. While it is in range and not more than 6 m
farther from the downed mate than they are, its mates hold off for the first 3 s, unless the
medic is the one down. An enemy within striking distance of the downed mate holds the medic
off like anyone else, however far away it still is. Once the medic goes, its mates cover it: ranged
mates with some teamwork engage whoever has the best view of its spot (`role` "cover"), and
melee mates stand 2 m off the spot on the side of the nearest enemy and take on whoever
comes within 5 m of it (`role` "screen"; `NpcBrain::guard` holds the spot). A squad without
a medic revives as before. A medic wears a bleached armband with a red cross over its left
brassard.

A medic also carries smoke (`smoke::SmokeClouds`). When an enemy has a clear view of a
downed mate's spot within 35 m, it throws a canister to land 2 m off the chest on that
enemy's side, one every 14 s at most. The cloud pops 0.9 s later, swells to 3.2 m over two
seconds, hangs and thins out over the last five of its 22 s. It hides the spot, so the medic
goes in as a skilled fighter does once a watched spot is out of view, and shots into it are
mostly guesses. With a smoke module installed (one that registers the "smoke" obscurance
source) the canister's gas goes to it as a `SmokeEmission`; without one the NPC module keeps a
plain cloud of its own under the "npc_smoke" source, drawn as many faint grey puffs (each a
few nested balls, so it fades out at its rim), bigger and fainter towards the cloud's edge and
drifting as it hangs.

| Setting | Default | Does |
| --- | --- | --- |
| `ally_revive_secs` | 2 s | How long a revive takes. 0 turns squad revives off. |
| `ally_revive_health` | 0.4 | The share of health a revived fighter gets up with. |
| `ally_revive_range` | 20 m | How far a mate goes to revive. |
| `revive_secs` | 0 (6 s in the demo) | The showcase timer. 0 turns it off. |

The showcase timer is the fallback. It waits while a mate is on the way or kneeling. When it
runs out, a fighter with a squad mate standing within `ally_revive_range` gets up where it lies
with `ally_revive_health`. Only a fighter with nobody near is sent back to its home, whole.

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
`LocalTimeScale` / `TimeDilation`, `LocomotionBackend`, `WorldShift`, `AppearanceBodies`,
`BodyRequest` (whose pose the animation module writes).

Writes: `MoveIntent` (the same input a player produces), `LocomotionState` and
`AnimationParams` (fallback mover only), `AttackEvent` (its own telegraphs), `DamageEvent`,
`ProjectileTrace` (its shots), `NoiseEvent` (shouts and gunfire), `GroundContact` and
`AnimationEvent` (footsteps), `WorldImpulse`, `IgniteRequest` and `SmokeEmission`
(explosions), `AppearanceRequest`, `SkeletonPose` and `FaceControls` (fighters), and
`Appearance::wear` (blood, bruises and dirt).

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
  land again, a sounder roots about and wanders together, boars run from a sprinter in the
  open, a boar cornered in a pen charges before it is trodden on and slashes, a wounded boar turns on
  whoever hurt it (the shooter, not what it was watching), stands and scents when it can't
  find who rather than charge at nothing, and runs from fire rather than
  charge into it, a sow charges to defend her piglets (a lone sow runs), wolves give room to
  a boar charging at them but not to one charging past (at six spots), wolves pick a piglet
  over its elders, a pack of three, four or five picks the wounded deer from the start, runs
  only it and catches it (unhurt, the herd's nearest), over twenty-four seeded hunts for
  each pack size (a test for each) its flankers run out on the quarry's sides (both on their
  own for 85 % of the flanking, abreast of it for a second in five of every eight hunts or
  more, never level with it on the far side) while the tester drives from behind and the ambusher lies in wait beyond the
  herd, a herd's alarm spreads within a reaction time (each deer looks toward the shot
  before it bolts, and the herd stays bunched), a deer runs straight from a wolf well back
  and zigzags once it is close, a deer stots at a wolf it sees but runs flat out from a bang,
  birds burst up at a gunshot 70 m off (not at an arrow), deer and boar look up at a gunshot
  60 m off and then walk off from it (and bolt from one 20 m off), an explosion 120 m off
  sends deer, boar and birds running (not a light thump, nor one out of earshot), a pack
  stalks in on a herd it scents from further off than
  it would run one down (`SCENT_REACH`), a pack gives up a chase after its time, all
  together, rests its time out though hungry, and hunts again, and breaks off sooner from a
  fit deer that outruns it (past its test, the deer more than `ESCAPE_GAP` ahead), deer and
  boar keep a day's hours by a pond (grazing in the morning, bedded down close together at
  midday, up in the afternoon, down to drink at dusk, each at its own place along the bank
  facing the water with its muzzle at the edge, bedded in the dark, up one by one at dawn
  and down to drink again), and a herd drinks at a stream's bank the same way. The test
  apps run each system in turn on one thread (`SingleThreadedExecutor`): far quicker than a
  thread pool for a handful of agents, and the same outcome, the order being fixed.
- `tests/alloc_free.rs`: `TacticalWorld::blocked_at`, which a fleeing animal calls for every
  way it might turn, finds walls, a pillar and a big slab and allocates nothing.
- `tests/demo.rs`: the arena fight keeps going for a minute (shots, attacks, blocks, cover,
  downed and revived) inside the arena, and over six fights with different randomness squad
  mates revive at least twice a minute on average and in every fight at least once; the meadow
  lives for two minutes (grazing, wandering, hunting, fleeing) without animals leaving.
- `tests/revive.rs`: a squad mate kneels beside a downed ally and gets it up where it lies,
  its hands on the chest for as long as the revive takes. Over 16 runs with different
  randomness: elites go in under a watching enemy's eyes in at most a tenth, veterans in at
  most half as many as rookies, rookies in nine tenths or more, and no skilled fighter goes in
  under fire; once the enemy looks away or is gone, the elite gets it up in every run. A mate
  revives an ally behind a wall rather than the nearer one in the open. A reviver that is hit
  lets go, stays put while it gets up and tries again only later. Nobody revives when it is
  off, or once it is turned off midway. A mate handed to game code does not hold up the
  showcase timer. The timer waits for a revive, gets a fighter with a mate near up in place,
  and sends home only one that is alone. A reviver going the long way round a wall is not
  taken for stuck, and nobody revives a mate on a floor above it. The squad's medic is the
  usual reviver, melee mates screen it from the enemy while it kneels, and a squad without one
  revives as before. In every one of 16 runs an elite medic smokes a watched mate and goes in
  under the smoke, hidden from the enemy as it kneels; only medics carry smoke.
- `tests/art_revive.rs`: a captioned strip, before and after. Before, a lone fighter lies six
  seconds and is sent home. After, a squad mate runs in, kneels, sets its carbine down, puts
  its hands on the chest and looks up at the enemy, and the ally gets up where it lay. It
  writes `fighters-revive-before-after.png`. A close-up from two sides
  (`fighters-revive-kneel-closeup.png`) shows the reviver getting down, its hands on the chest
  with its carbine set down clear of the mate's, and looking up. It checks that the mate kneels
  before the six-second timer and that the revive then takes no longer than the setting
  allows (from the hands on, and from the kneel with the 1.2 s getting down). It draws only
  with `NPC_SCREENSHOT_DIR` set; without it the scenes play out the same and every check runs.
  `tests/art_fighters.rs` likewise draws only the two pictures it counts pixels in.
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
- `tests/character.rs`:
  - Through a fight of swords, shield and carbine, the written `SkeletonPose` puts every limb
    segment, the head and the trunk where the stand-in has them, within 3 mm. Its chains hold
    together, and its names and parents are the animation module's.
  - Fighters ask for a look (their seed, one family per squad, a sex and an age range).
  - Their body hides only while `AppearanceBodies` exists, and the weapon and shield stay.
  - `FaceControls` follow pain (a blow, then the ache, worse badly hurt) and effort (a
    sprint, then panting).
  - Blows and a fall wear the look.
  - Each footstep is an `AnimationEvent` on its foot.
- `tests/schedule_order.rs`: no two systems in the fixed step or `PostUpdate` touch the same
  data without an order between them (the scheduler's ambiguity check), and the arena fight
  and the meadow end bit for bit the same with unrelated systems, resources or entities added.
  Randomness comes from each body's `Seed` (a hash of where it spawned and what it is, given
  on its first step; insert one to choose it), never from `Entity` ids, which every new
  resource moves.
- `tests/art_fighters.rs`, `tests/art_animals.rs`, `tests/art_arena.rs`: close-up renders of
  the fighters, wildlife and arena in chosen poses and lights, flipbook strips of the wildlife
  on the move (gaits, starts and stops, an alarm, a bend, a stalk, a wolf's hunt from stalk
  to bite, a boar rooting and its charge and slash, deer, wolves and boars flinching from a
  blow, a deer killed at a gallop, a starling's flight; `NPC_STRIPS=deer-walk,...` shoots
  only some, `NPC_STRIP_SCALE=2` twice the size), plus triangle and build-time budgets and a check that the arena art sits on
  every collider. In `tests/art_animals.rs` only the checks run by default (the budgets, and a
  few close-ups and strips rendered small, each of which must show its animal, the animals
  living on undrawn between shots); the full-size close-ups and strips are ignored tests
  (`-- --ignored`), and so are eight pictures of the AI at work, each one live run shot at
  eight moments: a pack's hunt from above with its parts marked in colour
  (`animal_live_pack_hunt.png`), the same kind of hunt from the grass at eye height, no
  markers, the wolves creeping crouched (`animal_live_pack_eye_level.png`), a chased deer
  zigzagging seen from low behind
  (`animal_live_zigzag.png`), a herd's alarm spreading (`animal_live_herd_alarm.png`) and a
  sounder rooting, then its tusker charging a wolf that bit it, from the grass with no
  markers (`animal_live_boar_eye_level.png`), and a flock of starlings bursting up at a
  gunshot, from the grass (`animal_live_birds_gunshot.png`), a herd moving out at dusk and
  down to a pond to drink, from the far bank (`animal_live_deer_drinking.png`), and a herd
  bedding down at midday, chewing the cud and dozing (`animal_live_deer_bedded.png`). Each walking animal's close-up is a still from a real run at the game's
  rate (it walks on toward the camera's mark and is shot at its moment), so its legs are
  where its gait has them. The animal unit tests check that no planted foot slides in any
  gait by more than a millimetre a frame (boars, tuskers and piglets included, deer as they
  bolt and pull up, a rabbit's paws even as they peel off at a run), that a stotting deer
  bounds high on all fours and pronks (legs straight, back arched, feet off and down
  together, rump patch flared, the body within 0.12 rad of level and 0.06 in the air, the
  head held high) and that a rabbit's fore paws never land out of reach, that a rooting boar's snout reaches the soil, that a tusker's upper tusks stay in its
  lip as its jaw opens, that a hurt animal jolts away from the blow, pins its ears, stumbles,
  steps, hitches in its stride and settles, its head swinging well away from a blow from
  either side, its body turning only a little (the front feet stepping out instead, no
  planted foot sliding), and that the flinch does not jump when the state
  changes, that heads turn to a threat and to a noise before the neck, and that a shot
  starling lands as it was tumbling.
- `tests/screenshots.rs`: renders the arena, its stands, a cover fight, the sparring lane, the
  duel and the meadow on a GPU or lavapipe and checks the fighters and deer show up.
  `NPC_SCREENSHOT_DIR=dir` keeps the PNGs.

## Done when

- Enemies that feel smarter than other games: they take cover, flank, punish mistakes, and block.
- Dynamic fauna roams, herds, drinks, rests and flees from threats, fire and explosions.
- The showcase arena shows all of it with readable bodies and an intent overlay.
