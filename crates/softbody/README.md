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
- **Collision** with physics colliders (sphere, capsule, box, cylinder; convex hulls as boxes; compounds), character capsules, capsules on skeleton bones (`BoneColliders`) and the `Ground`. Collider poses are swept through the substeps, so fast colliders don't tunnel. Coulomb friction.
- **Two-way coupling**: reaction impulses on dynamic colliders become `ApplyImpulse` (cloth pushes crates, jelly sits on them with its full weight); ropes tied to dynamic bodies pull them; characters and `SoftLoad`s weigh rope bridges down.
- **Cloth** (`Cloth`): flags, curtains (gathered into folds), capes and clothing pinned to bones; per-triangle aerodynamics (pressure drag and skin friction in the world wind with gusts), tearing when overstretched, bullet holes (`ProjectileTrace`), fire that spreads upward and burns triangles away (`IgniteRequest`, `HeatField`, explosions with heat), rain soaking it heavier and darker. The drawn cloth shows fine wrinkles wherever the solver reports compression, printed patterns, fabric sheen, charring and glowing burn edges.
- **Ropes** (`Rope`) and **rope bridges** (`RopeBridge`): ends tied to nothing, the entity, a world point or another entity; cuttable by bullets, `softbody.cut` or code. Bridge planks are kinematic colliders driven by the ropes, so walkers stand on the sagging deck.
- **Soft volumes** (`SoftVolume`): a particle lattice with one shape-matching constraint (rotation extraction, Müller 2016) plus lattice springs; plasticity keeps dents. A smooth rounded mesh is embedded in the lattice.
- **Hair and fur** (`HairGroom`): guide strands grow from a scalp ellipsoid or around bone capsules, draped over the skin, and are simulated as tiny ropes with goals toward their combed shape (stiffer at the root), tethers, drag and wind, colliding with the body. The renderer grows 30–60 drawn strands around each guide in the vertex shader (Catmull-Rom, spread, clumping, curl, per-strand length) with two-lobe Kajiya-Kay shading; strands thin out with distance. Melanin and redness give natural colors.
- **Time**: runs in `FixedUpdate` / `SimSet::PostPhysics` (after rigid bodies). Bodies on a character use that character's `LocalTimeScale`, so a slowed character's cape and hair move in slow motion too.
- **LOD**: bodies farther than `SoftBodyConfig::lod_distance` from every `SimulationFocus` step every other fixed step; past `freeze_distance` they hold still. Bodies are culled for drawing and shadows separately.

## Components, resources and API

| Name | What |
|---|---|
| `Cloth`, `Rope`, `RopeBridge`, `SoftVolume`, `HairGroom` | Specs. The plugin adds a `SoftBodySim` next to each (or `SoftBodyFailed` with a readable reason). |
| `BoneColliders` | Capsules on a character's bones for cloth and hair (`humanoid()` for the usual bone names). |
| `SoftLoad` | Weight an entity puts on rope bridges without a physics character. |
| `SoftBodySim` | Runtime state: particles, constraints, bounds, step time. Read it for positions. |
| `SoftBodyConfig` | `enabled`, `lod_distance`, `freeze_distance`, `quality` (substep multiplier). |
| `softbody.list` / `stats` | What exists and what it costs. |
| `softbody.cut` / `shoot` / `ignite` / `push` / `unpin` / `reset` | Act on bodies by world position or entity. |

## Contracts

Reads: `WindField`, `Gravity`, `Ground`, `WeatherState` (rain), `HeatField`, `FocusPoints`, `ProjectileTrace`, `WorldImpulse`, `IgniteRequest`; physics contract (`Collider`, `RigidBody`, `MassProperties`, `CharacterController`, `CharacterState`); animation contract (`SkeletonPose`); `LocalTimeScale`.

Writes: `ApplyImpulse` (soft bodies pushing and pulling rigid bodies), `GroundContact` (landings), `NoiseEvent` (impacts), `SmokeEmission` (burning cloth), its own components, and draws through `SoftBodyFeature` (a `RenderFeature`).

No dependencies on other module crates (see ARCHITECTURE.md).

### Strand data for the shared strand renderer

Hair and fur are drawn by this crate until the shared strand renderer exists. The format it needs is in `src/shaders/strand_common.wgsl` and `render::Geometry::add_hair`: a point buffer (guide particles, camera-relative), one `GpuStrandGuide` per guide (first point, count, groom, seed, root normal and tangent, length) and one `GpuGroom` per groom (color and thickness, spread, clumping, curl, strands per guide, segments, length variation, root shading). Grass can use the same buffers with sway instead of simulation.

## Demo

`demo::spawn_demo(commands, origin)` builds the yard: five banners and a field of 56 pennants, a curtain shot full of holes and one that burns away and comes back, a long rope bridge with a walker and a short one that is cut and drops, a bead curtain a ball pushes through, a swing, jelly cubes a wrecking ball knocks around, clay a press dents, a caped mannequin with long hair, the same mannequin in slow motion, and a dog with fur. Until the physics and animation modules land, props are kinematic and the skeletons are animated procedurally here through the same `SkeletonPose` contract.

```sh
cargo run -p engine_softbody --example yard --release -- out_dir 1280x720   # close-ups of each piece
cargo run -p showcase --release -- --headless --tour yard                  # the showcase viewpoint
```

## Done when

- [x] A cape on a running character stays on the body without jitter or tunneling, and reacts to wind and to slow motion.
- [ ] A flag field (50+ flags) costs under 1 ms on the GPU: 56 pennants run on the CPU today; GPU compute is next.
- [ ] A sail pushes a boat: needs the physics backend (cloth already sends `ApplyImpulse`).
- [x] A rope bridge carries a walker and swings; cutting a rope drops it.
- [x] A jelly cube wobbles, rests on crates and gets knocked over (by a kinematic wrecking ball until rigid crates can move).
- [x] A bullet through a curtain leaves a hole; fire burns cloth away.
- [x] Long hair on a dashing head swings and settles without passing through the head; a dog's fur moves with it; both hold up in slow motion.

Next: GPU compute for large visual cloth, self-collision, water (buoyancy and wet cloth from `Water`), trimesh colliders through `PhysicsQueries`, hair wetness clumping.
