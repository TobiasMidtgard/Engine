# engine_softbody — Cloth, ropes, soft bodies, hair and fur

**Owner:** Cloth, soft bodies and hair. **Status:** first release: everything below runs, is tested and is in the showcase yard.
Background research: `game-engine-research/physics.md` (cloth and soft-body sections) in the project files.

Rigid bodies live in the physics module; this crate simulates everything that bends, stretches or squishes and couples it to rigid bodies both ways through the physics contract only, so either module can be replaced.

```rust,ignore
app.add_plugins(SoftBodyPlugin);

// A flag on a pole (left edge pinned), streaming in the world wind.
commands.spawn((Transform::from_xyz(0.0, 8.8, 0.0), Cloth::flag([3.0, 2.0], SoftMaterial::hex(0xc0392b))));
// A curtain gathered into folds on rings; bullets leave holes, fire burns it away.
commands.spawn((Transform::from_xyz(0.0, 6.0, 0.0), Cloth::curtain([4.6, 5.6], SoftMaterial::hex(0x7b1e2b))));
// A cape on a character (child of the entity with the SkeletonPose).
commands.spawn((Transform::from_xyz(0.0, 1.46, 0.21), Cloth::cape([0.72, 1.2], red, &["chest"]), ChildOf(hero)));
// Long hair and a body that cape and hair collide with.
commands.entity(hero).insert(BoneColliders::humanoid());
commands.spawn((HairGroom::long_hair("head", HairStyle::long()), ChildOf(hero)));
// A rope from here to a crate (two-way: a dynamic crate gets pulled).
commands.spawn((Transform::from_xyz(0.0, 4.0, 0.0), Rope { end_b: RopeEnd::Entity { entity: crate_, offset: [0.0; 3] }, ..default() }));
// A walkable rope bridge along local +X, and a jelly cube.
commands.spawn((Transform::from_xyz(0.0, 3.0, 0.0), RopeBridge { length: 19.0, ..default() }));
commands.spawn((Transform::from_xyz(4.0, 1.0, 0.0), SoftVolume::jelly(0.8, SoftMaterial::hex(0x2ecc71))));
```

Every spec component is data (serde, registered with docs), so AI agents and the editor can create and tune them; changing a spec rebuilds the simulation.

## What it does

- **One XPBD solver** (`solver.rs`): particles and distance, tether, pin, goal and shape-matching constraints with small substeps (Macklin et al. 2019), one iteration per substep. Fewer substeps in slow motion, so slow motion is cheaper and moves proportionally less.
- **Collision** with physics colliders (sphere, capsule, box, cylinder; convex hulls as boxes; compounds; triangle meshes and height grids, see below), character capsules, capsules on skeleton bones (`BoneColliders`) and the `Ground`. Collider poses are swept through the substeps, so fast colliders don't tunnel. Coulomb friction.
- **Mesh colliders** (`field.rs`): a `TriMesh` collider is baked into a narrow-band signed distance grid (1.5 cm cells, coarser past 2.4 m; real distances within 4.5 cm of the surface, inside and outside told apart by angle-weighted pseudo-normals, so edges and corners stay right) once a soft body comes within 10 m of it, on all cores, and on a thread of its own for meshes over 4096 triangles (the collider takes part once its grid is ready). Particles sample it trilinearly with its gradient as the normal, as cheaply as a box. Open meshes (a sheet, a terrain skin) work: their back side counts as inside, near the surface only. A changed or rescaled collider bakes again. `Heightfield` colliders are sampled directly (bilinear height, slope normal). Cloth drapes over a statue's raised arm and turned head this way, not over stand-in capsules (`tests/drape.rs`, `tests/unveil.rs`).
- **Two-way coupling**: reaction impulses on dynamic colliders become `ApplyImpulse` (cloth pushes crates, jelly sits on them with its full weight); ropes tied to dynamic bodies pull them; characters and `SoftLoad`s weigh rope bridges down.
- **Cloth** (`Cloth`): flags, curtains (gathered into folds), capes and clothing pinned to bones, and sails (`Cloth::sail`: a four-sided panel with a sewn-in belly, laced along its luff, head and foot); per-triangle aerodynamics (pressure drag, skin friction and, for sails, lift across the wind, in the world wind with gusts). Cloth on an entity carried by a dynamic rigid body hangs on stiff goals whose reactions push that body, so a sail drives and heels its boat. Self-collision (`SimSettings::self_collide`, on for curtains and capes) keeps folds and heaps in layers, with friction between them, at about a tenth of the step's cost once the cloth settles. Tearing when overstretched, bullet holes (`ProjectileTrace`), fire that spreads upward and burns triangles away (`IgniteRequest`, `HeatField`, explosions with heat; soaked cloth won't catch). The drawn cloth shows fine wrinkles wherever the solver reports compression, printed patterns (stripes, crosses, borders, and dyed Japanese motifs: seigaiha waves, plum blossoms, kanoko tie-dye dots), fabric sheen, charring and glowing burn edges.
- **Water** (from the world's `Water`): each body samples the surface and current over itself every step (a 3 × 3 grid of heights, so a long body follows a sloping surface, and one mean current). Under the surface a particle is lifted against gravity (`SimSettings::buoyancy`, a fraction of its dry weight, eased in across its thickness) and drawn toward the current, implicitly so it is stable however strong (`water_drag`); cloth resists moving across its face far more than sliding along it, so a sinking sheet glides and a strip in a stream lies along the flow. The wind only reaches what is out of the water. Moving water swirls with eddies carried along by the current, the same for bodies side by side, so silks in a stream undulate and roll. Each particle has its own wetness (`SoftBodySim::wet`): it soaks through in about 4 s under water, in the rain at a rate set by the rainfall, wicks a few centimeters along the fabric or strand past the waterline, drains down a hanging body, and dries in wind and warmth. Soaked particles weigh up to (1 + `soak`) times as much, so dry cotton floats until it soaks through and then sinks, and wet hair hangs heavy. Drawn wet, cloth darkens and deepens in color, turns glossy and loses its sheen, with a wet line where it meets the water; hair darkens, gathers into clumps and turns glossy.
- **GPU cloth** (`SimSettings::gpu`): flags, banners and pennants that only need to look right run in a compute shader instead (`gpu_cloth.rs`, `shaders/cloth_sim.wgsl`), one workgroup per cloth with its grid (up to 512 particles; larger grids are thinned) in workgroup memory for the frame's substeps: gravity, gusting wind with the same pressure drag and skin friction as the CPU cloth, and XPBD stretch, shear and bend links solved in twelve colours of independent links. The kernel writes the vertices the regular cloth pipelines draw, with shadows, so a GPU flag looks like a CPU one and flies like one (`tests/gpu_flags.rs` compares them). Rain soaks them as it does CPU cloth (one wetness per cloth, the hem drawn a little wetter), and soaked they weigh more, so the wind lifts them less; wind and warmth dry them. No water, collisions, self-collision, tearing, holes or fire; rectangles with edge or top-corner pins only. They follow world shifts and the time of the frame (`GpuClothProbe` reads one back for tests).
- **Ropes** (`Rope`) and **rope bridges** (`RopeBridge`): ends tied to nothing, the entity, a world point or another entity; cuttable by bullets, `softbody.cut` or code. Bridge planks are kinematic colliders driven by the ropes, so walkers stand on the sagging deck.
- **Soft volumes** (`SoftVolume`): a particle lattice with one shape-matching constraint (rotation extraction, Müller 2016) plus lattice springs; plasticity keeps dents. A smooth rounded mesh is embedded in the lattice.
- **Hair and fur** (`HairGroom`): guide strands grow from a scalp ellipsoid, around bone capsules, or over a whole skinned body mesh (`HairEmitter::Surface`: roots spread evenly by area, each riding the bones that carry the skin under it (`MeshSkins`; blended like the skinning at the joints, so the coat stays on bending skin), combed along the skeleton, longer or shorter by `FurZone`), draped over the skin, and are simulated as tiny ropes with goals toward their combed shape (stiffer at the root), tethers, drag and wind, colliding with the body. The renderer grows 30–60 drawn strands around each guide in the vertex shader (Catmull-Rom, spread, clumping, curl, per-strand length) with two-lobe Kajiya-Kay shading; strands thin out with distance. Melanin and redness give natural colors.
- **Time**: runs in `FixedUpdate` / `SimSet::PostPhysics` (after rigid bodies). Bodies on a character use that character's `LocalTimeScale`, so a slowed character's cape and hair move in slow motion too.
- **LOD**: bodies farther than `SoftBodyConfig::lod_distance` from every `SimulationFocus` step every other fixed step; past `freeze_distance` they hold still. Bodies are culled for drawing and shadows separately.

## Components, resources and API

| Name | What |
|---|---|
| `Cloth`, `Rope`, `RopeBridge`, `SoftVolume`, `HairGroom` | Specs. The plugin adds a `SoftBodySim` next to each (or `SoftBodyFailed` with a readable reason). |
| `BoneColliders` | Capsules on a character's bones for cloth and hair (`humanoid()` for the usual bone names). |
| `SoftLoad` | Weight an entity puts on rope bridges without a physics character. |
| `MeshSkins` | Skin weights of skinned meshes in `Assets<Mesh>`, by key, so `Surface` fur moves with the skin. |
| `SoftBodySim` | Runtime state: particles, constraints, bounds, step time. Read it for positions. |
| `SoftBodyConfig` | `enabled`, `lod_distance`, `freeze_distance`, `quality` (substep multiplier). |
| `SimSettings::buoyancy`, `water_drag`, `soak` | In water: lift on it fully under, as a fraction of its dry weight; how fast it takes on the current (1/s); and the water it soaks up, as a fraction of its dry weight. Cloth 1.6, 6, 1.5 (cotton); hair 0.85, 9, 0.5; rope 0.9, 6, 0.8; rope bridge (timber planks) 1.5, 6, 0; jelly 0.95, 1.5, 0. |
| `SimSettings::gpu` | Simulate this cloth on the GPU (visual only, see above). `render.stats` shows `gpu_cloths` and the pass's GPU time `gpu_cloth_ms` under the `softbody` feature. |
| `GpuClothProbe` | Name a GPU cloth and read its particles back each frame (stalls the frame: tests and tools only). |
| `softbody.list` / `stats` | What exists and what it costs. |
| `softbody.cut` / `shoot` / `ignite` / `push` / `unpin` / `reset` | Act on bodies by world position or entity. |
| `softbody.sailboat_home` | Send the self-sailing dinghies back to their moorings to start their lanes over (tour stops use it). |
| `softbody.unveil` | Start the unveilings over: a fresh sheet high on the crane, lowered over the statue a few seconds later (tour stops use it). |

## Contracts

Reads: `WindField`, `Gravity`, `Ground`, `Water` (surface and current), `WeatherState` (rain, temperature), `HeatField`, `FocusPoints`, `ProjectileTrace`, `WorldImpulse`, `IgniteRequest`, `WorldShift` (particles, GPU cloths, collider history and world-anchored rope ends follow the world into its new frame in `WorldShiftSet::Apply`); physics contract (`Collider`, `RigidBody`, `MassProperties`, `CharacterController`, `CharacterState`); animation contract (`SkeletonPose`); `LocalTimeScale`.

Writes: `ApplyImpulse` (soft bodies pushing and pulling rigid bodies), `GroundContact` (landings), `NoiseEvent` (impacts), `SmokeEmission` (burning cloth: about 0.1 g a puff, 1.5 puffs a second per burning triangle, a few hundredths of a kilogram a second for a whole curtain), its own components, and draws through `SoftBodyFeature` (a `RenderFeature`).

No dependencies on other module crates (see ARCHITECTURE.md).

### Strand data for the shared strand renderer

Hair and fur are drawn by this crate until the shared strand renderer exists. The format it needs is in `src/shaders/strand_common.wgsl` and `render::Geometry::add_hair`: a point buffer (guide particles, camera-relative), one `GpuStrandGuide` per guide (first point, count, groom, seed, root normal and tangent, length) and one `GpuGroom` per groom (color and thickness, spread, clumping, curl, strands per guide, segments, length variation, root shading). Grass can use the same buffers with sway instead of simulation.

## Demo

`demo::spawn_demo(commands, origin)` builds the yard: five banners and a field of 56 pennants simulated on the GPU, a curtain shot full of holes and one that burns away and comes back, a long rope bridge crossed back and forth by the mannequin in a wool cloak that streams out in the breeze (walking pace, easing into a stepping turn at each tower) and a short one that is cut and drops, a bead curtain a ball pushes through, a swing, jelly cubes a wrecking ball knocks around, clay a press dents, a wool blanket dropped over a stone ball that heaps up in layers, a caped mannequin with long hair, the same mannequin in slow motion, a dog with fur, and an unveiling: a timber jib crane lowers a crimson silk sheet over a marble youth after Michelangelo's David on a sandstone plinth, leaves it draped, and lifts it off again (`demo::statue`; the statue is the mannequin's body carved deeper as a statue, `BodyPart::Statue`: abdomen, ribs, hip grooves, biceps, the heads of the thigh, open eyes and snail-shell curls, the raised hand bent back at the wrist, in veined Carrara marble mapped without UVs, `Mat::Marble`; it is skinned once into its pose, and its posed surface is the mesh collider the silk drapes over). The curtain burner's flame is an `IgniteRequest`; a world with a fire module keeps the ground bare around `demo::CURTAIN_BURNER` (the showcase does), or the flame lights the grass too. `demo::spawn_sailboat(commands, home, range)` launches a self-sailing gaff dinghy (modeled clinker hull, rig and rudder from `demo::boat_models`) that trims its sheet, steers with its rudder and sails back and forth across the wind; it floats on physics water and waits at its mooring where there is none. `demo::rinse::spawn_rinse(commands, at, downstream, span)` lays a bamboo pole across a stream on crossed stakes with five kimono silks tied to it (yuzen nagashi, as Kyoto's dyers rinsed theirs in the Kamo river): once the stream under it is known it settles a hand above the water and ties them on, and they stream out soaked, rolling in the eddies. The showcase puts it in the valley stream above the falls (tour stop `yard_rinse`; the stream is the lake zone's, so render it with `--tour yard_rinse --zones yard,lake`). Until the physics and animation modules land, props are kinematic and the skeletons are animated procedurally here through the same `SkeletonPose` contract.

The characters are seamless sculpted bodies (`demo::body`, `demo::sculpt`): a signed distance field of lofted limbs and torso, muscles, face and paws, blended smoothly, meshed once at 3.4–4 mm (surface nets, projection onto the surface, curvature-aware decimation to about 145k triangles for the mannequin and 120k for the dog, relaxation, baked ambient occlusion) on background threads and shared. `demo::skin` deforms each body on the CPU every frame with dual-quaternion skinning (up to four bones per vertex, no candy-wrapper twists) into a mesh asset in well under a millisecond. The mannequins are satin fiberglass (ivory, graphite); the dog's coat grows over its whole skinned body.

```sh
cargo run -p engine_softbody --example yard --release -- out_dir 1280x720   # close-ups of each piece
cargo run -p engine_softbody --example unveil --release -- out_dir 1280x720 [shot,...] # the bare statue, then the unveiling through one cycle
cargo run -p engine_softbody --example rinse --release -- out_dir 1280x720 [shot,...]  # the silks in a stand-in stream
cargo run -p showcase --release -- --headless --tour yard                  # the showcase viewpoint
```

## Done when

- [x] A cape on a running character stays on the body without jitter or tunneling, and reacts to wind and to slow motion.
- [ ] A flag field (50+ flags) costs under 1 ms on the GPU: the yard's 56 pennants (240 particles each) run on the GPU in one dispatch and cost the CPU nothing. Software Vulkan (llvmpipe, on the CPU) takes about 40 ms for 64 flags; the time on a real GPU (`gpu_cloth_ms` in `render.stats`) is still to be read.
- [x] A sail pushes a boat: the dinghy's sail drives and heels it through the physics backend (`tests/sail.rs`), and it sails a lane across the wind on a lake.
- [x] A rope bridge carries a walker and swings; cutting a rope drops it.
- [x] A jelly cube wobbles, rests on crates and gets knocked over (by a kinematic wrecking ball until rigid crates can move).
- [x] A bullet through a curtain leaves a hole; fire burns cloth away.
- [x] Long hair on a dashing head swings and settles without passing through the head; a dog's fur moves with it; both hold up in slow motion.
- [x] Cloth and hair in water: dry cloth floats, soaks through and sinks; a dipped cloth is wet below the waterline and a little above; currents carry cloth and sweep hair downstream; rain soaks cloth and wind dries it (`tests/water.rs`); silks stream out in the showcase's running stream.

Next: splashes and ripples where cloth meets the water (through the water module's contract).
