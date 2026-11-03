Round 21d step 3: the fixed step chain at the plaza, main 1810b539, Tobias's PC (RTX 5080, 32 threads), release, Ultra,
window as the exe opens it (1600x900). ENGINE_PROFILE=1 (no --profile flag), PHYSICS_REPORT=600 in every run.
physics.* engine.stats and engine.status (frame, fixed_tick) sampled twice a second through the bridge.

3a, as asked: showcase.exe --tour plaza --fly --quality ultra (tour-plaza/). It ran 17 s and exited by itself:
"tour 1/1: The market square and the village", "waited 6 s for meshes' clusters, terrain patches and foliage", exposure
EV100 13.5, picture, exit. --tour plaza loads only the plaza's zone (zones::demo_zones takes the tour's stops), so
physics had 0 bodies (physics.step_ms 0.04-0.06, 1 region, 0 contacts), and the tour steps time by hand at 0.05 s a
frame, so fixed steps are 3.00 a frame by construction (432 ticks over 144 frames). 432 steps < 600: no physics
report. Its 3 frame profiles are startup (stream_far_woods 500 ms a frame in one, 27 ms in the last; water ripple 25 ms
in FixedUpdate's chain in the first).
That can't answer the step-chain question, so the three PHYSICS_REPORT runs are ~60 s real-time stands at the plaza in
the full world instead: showcase.exe --fly --quality ultra, showcase.goto plaza 5 s after the world is built, stay 75 s
(the first ~20 s are the startup) (plaza-run1..3/).

Steady state = frame profiles 25-75 s after arriving (11 per run):
run | fps | physics_simulate ms/frame (ran alone) | fixed steps a frame (profile runs) | FixedUpdate floor of FixedUpdate ms | physics report ms per step (Rapier) | physics.step_ms median / p90 (whole stay)
1 | 26.1-32.4 | 3.72-8.30 (1.95-5.14) | 1.8-2.3 | 10.1-14.0 of 14.7-19.3 | 2.80-4.08 (1.74-2.51) | 3.74 / 4.88
2 | 29.3-34.8 | 1.85-2.23 (0.50-0.75) | 1.7-2.0 | 6.9-8.7 of 12.1-15.0  | 2.00-2.71 (0.68-0.71) | 1.26 / 2.52
3 | 30.2-33.8 | 2.23-3.22 (0.81-1.38) | 1.8-2.3 | 6.9-11.6 of 12.5-17.9 | 2.10-2.72 (0.90-1.14) | 1.51 / 2.43
engine.status over the whole stay: run 1 2.14 fixed steps a frame at 25.9 fps, run 2 1.90 at 29.4, run 3 1.94 at 28.6.
The first report of each run (05:47:08 / 05:49:33 / 05:51:59, 7.65-8.98 ms per step, "the rest (sync, upkeep, write back)"
4.1-5.2) covers the startup steps (physics.step_ms samples up to 85-192 ms in the first ~10 s), and has 22 water samples a step;
the steady reports have 14-15 water samples a step, 2 buoyant colliders, 1 substep.

Ordering floor (ordering_floor.txt has whole blocks), steady profile of run 2 (05:50:19, 34.1 fps):
  Render       9.14 of 9.18: render_system 9.13
  FixedUpdate  8.10 of 12.73, chain: npc::rebuild_world_model 0.08 -> npc::think_and_act 0.38 -> water::boat_air_system
    0.75 -> vehicles::wheels_system 0.16 -> vehicles::engine_events_system 0.06 -> physics::physics_sync 0.37 ->
    physics::physics_simulate 2.04 -> physics::physics_write_back 0.18 -> softbody::gather_colliders 0.44 ->
    softbody::step_bodies 0.92 -> softbody::write_posed_guides 0.20 -> water::sync_bodies_system 0.10 ->
    water::flow_bed_system 1.85 -> water::disturbance_system 0.08 -> water::shore_wetness_system 0.05
  PostUpdate   1.85 of 2.94, chain: terrain::deform_from_contacts 0.11 -> terrain::wear_from_contacts 0.10 ->
    space::update_space 0.06 -> core::propagate_transforms_system 0.41 -> vfx::tick_effects 0.58 -> anim::foot_ik_system
    0.29 -> anim::ragdoll_system 0.06
  Update       1.37 of 3.50, chain: script::run_scripts 1.13 -> anim::evaluate_animators 0.09
Run 1 (05:47:54, 30.1 fps): the same FixedUpdate chain at 11.72 of 16.28 with physics_simulate 4.98 and flow_bed 1.86;
Render 9.19, PostUpdate 2.00 of 3.12, Update 1.47 of 3.70. Run 3 (05:52:45): FixedUpdate 9.17 of 14.04 (simulate 2.77).
FixedUpdate runs ~2 times a frame, so its floor is ~2 x the chain's per-step cost: physics_simulate and flow_bed_system
are its two heavy links in every run.

Run 1 is the high one. Its "awake:" lists and water samples (physics_reports.txt has all six reports of each run):
the region with the Motorboat and the Rowboat 359 m out (8 bodies, 2 awake: Motorboat 5.7 m/s at (-353, 6, 29), Rowboat
0.0-0.1 m/s; 1636 colliders, 13 pairs, 9 touching; most pairs a terrain collider tile and three destruction Debris
pieces) costs 1.33-3.02 ms a step in run 1's six reports (1.95 in the last) against 0.31-0.35 in run 2's, with the same bodies awake. In run 1's last report
"Rapier" is 2.25 ms a step but Rapier's own stages add up to 0.25 (broad 0.05, narrow 0.05, solver set-up 0.02, solving
0.11, user changes 0.02); in run 2 Rapier 0.71 against the same 0.25. So ~2 ms a step goes inside the Rapier call, but
outside its named stages, in that region. 14 water samples a step in both. Run 1's other awake regions: the plaza's
(78 awake of 91: Wrecking ball 5.6 m/s kinematic, a Great sword dummy ragdoll, Swing seat) 0.44 ms; cars and planes
(Circuit plane 54.6 m/s, Blue car 28.4, Red car 25.1, Sailing dinghy) 0.40 ms.
This looks like the loops' round 2 (step 2a: rapier step 1.6-3.7 ms each instead of 0.2-0.6, with no PHYSICS_REPORT
there).

Files: README.txt, ordering_floor.txt; per run: analysis.txt (every frame profile's schedules, physics_simulate, the
ordering floor block; physics.step_ms and fixed steps per 5 s; the physics reports), physics_reports.txt (plaza runs),
frames.csv, sampled_stats.jsonl (physics.* twice a second), timeline.txt, nvidia_dmon.txt, the plaza picture,
logs.zip (stderr, engine-profile.txt).
