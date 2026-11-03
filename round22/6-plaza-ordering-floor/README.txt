Round 22 step 6: plaza stand in the full world, main 5de4f33e, Tobias's PC (RTX 5080, 32 threads), release, Ultra,
window 1600x900. ENGINE_PROFILE=1 showcase.exe --fly --profile --quality ultra; showcase.goto plaza 5 s after the world
was built (19:19:50 UTC), stay 75 s. physics.* engine.stats and engine.status (frame, fixed_tick) twice a second.
Steady state = the 11 frame profiles 25-75 s after arriving (19:20:15-19:21:05): 29.8-36.5 ms a frame (27.4-33.5 fps);
engine.status over that span 31.6 fps, 1.89 fixed steps a frame; physics.step_ms 1.14-1.44 mean per 5 s (max 2.10);
the first ~10 s after arriving are the startup (physics.step_ms up to 202.8, 1.7-4 fps).

Ordering floor (ordering_floor.txt has the block of all 11 steady profiles; analysis.txt every profile of the run).
Steady ranges, floor of the schedule's ms per frame:
  Render         8.33-10.23 of  8.37-10.28
  FixedUpdate    7.15-9.88  of 12.71-17.14 (runs 1.8-2.2 a frame)
  PostUpdate     1.90-2.36  of  3.00-3.71
  Update         1.33-1.71  of  3.55-4.44
  First          0.20-0.25  of  0.28-0.34
  Last           0.19-0.27  of  0.28-0.37
  Extract        0.14-0.17  of  0.18-0.22
  FixedPreUpdate 0.12-0.15  of  0.16-0.20
(PreUpdate and FixedPostUpdate are not in the block: 0.12-0.15 and 0.03-0.05 ms a frame. Last, Extract and
FixedPreUpdate are one system each and print no chain line.)

The block of the 19:20:40 profile (33.5 fps, 29.8 ms a frame), as printed:
  Render             9.22 of   9.26 (2 systems; heaviest: engine_render::render_system 9.22)
    chain: engine_render::render_system 9.22
  FixedUpdate        8.64 of  12.71 (65 systems; heaviest: engine_water::flow_bed_system 2.24, engine_physics::physics_simulate
    1.96, engine_softbody::step_bodies 0.95, engine_water::boat_air_system 0.76, engine_npc::think_and_act 0.56,
    engine_softbody::gather_colliders 0.43, engine_physics::physics_sync 0.39, engine_softbody::write_posed_guides 0.20)
    chain: engine_npc::rebuild_world_model 0.08 -> engine_npc::think_and_act 0.56 -> engine_water::boat_air_system 0.76 ->
    engine_vehicles::wheels_system 0.19 -> engine_vehicles::engine_events_system 0.07 -> engine_physics::physics_sync 0.39 ->
    engine_physics::physics_simulate 1.96 -> engine_physics::physics_write_back 0.16 -> engine_softbody::gather_colliders
    0.43 -> engine_softbody::step_bodies 0.95 -> engine_softbody::write_posed_guides 0.20 -> engine_water::sync_bodies_system
    0.10 -> engine_water::flow_bed_system 2.24 -> engine_water::disturbance_system 0.08 -> engine_water::shore_wetness_system 0.05
  PostUpdate         1.93 of   3.11 (33 systems; heaviest: engine_vfx::tick_effects 0.57, engine_core::propagate_transforms_system
    0.42, engine_anim::foot_ik_system 0.33, engine_terrain::deform_from_contacts 0.11, engine_terrain::wear_from_contacts 0.11,
    engine_space::update_space 0.07, engine_anim::ragdoll_system 0.06)
    chain: engine_terrain::deform_from_contacts 0.11 -> engine_terrain::wear_from_contacts 0.11 -> engine_space::update_space
    0.07 -> engine_core::propagate_transforms_system 0.42 -> engine_vfx::tick_effects 0.57 -> engine_anim::foot_ik_system 0.33
    -> engine_anim::ragdoll_system 0.06
  Update             1.33 of   3.62 (32 systems; heaviest: engine_script::run_scripts 1.08, engine_anim::evaluate_animators 0.10)
    chain: engine_script::run_scripts 1.08 -> engine_anim::evaluate_animators 0.10
  First              0.22 of   0.31 (4 systems; heaviest: engine_core::frame_diagnostics_system 0.19)
    chain: engine_core::frame_diagnostics_system 0.19
  Last               0.22 of   0.32 (1 systems; heaviest: engine::stream_system 0.22)
  Extract            0.15 of   0.19 (1 systems; heaviest: engine_render::resolve_mesh_renderers 0.15)
  FixedPreUpdate     0.12 of   0.16 (1 systems; heaviest: engine_core::snapshot_interpolation_system 0.12)
Against 21d step 3 run 2 (05:50:19, 34.1 fps): Render 9.14 of 9.18; FixedUpdate 8.10 of 12.73 with the same chain
(physics_simulate 2.04, flow_bed_system 1.85, step_bodies 0.92, boat_air_system 0.75); PostUpdate 1.85 of 2.94; Update
1.37 of 3.50. The FixedUpdate chain is unchanged in shape; flow_bed_system (1.40-2.45 a frame here) and physics_simulate
(1.96-2.35) stay its two heavy links, x ~1.9 runs a frame.

Files: ordering_floor.txt, analysis.txt (21d's tour script: every profile's schedules, physics_simulate and ordering
floor block; physics.step_ms and fixed steps per 5 s), sampled_stats.jsonl (physics.*), frames.csv, timeline.txt,
nvidia_dmon.txt, plaza.jpg, logs.zip (stderr, engine-profile.txt).
