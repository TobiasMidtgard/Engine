//! `gpu_sweep`: what each part of the frame costs on the GPU, measured on a running game.
//!
//! Start the game with its bridge and the frame profiler, then run the sweep against it:
//!
//! ```sh
//! ENGINE_BRIDGE=1 showcase --fly --profile --quality ultra &
//! cargo run --release -p engine-mcp --bin gpu_sweep -- \
//!     --stops plaza,forest_trail,lake,overview --out gpu-sweep
//! ```
//!
//! At each stop it waits for the view to settle, reads every `render.gpu.<pass>` row for a few
//! seconds (the baseline), then switches one thing at a time (a `RenderSettings` field, a weather
//! or terrain setting, or settings of two modules together), reads the rows again and switches it
//! back. The difference between a variant's total and the baseline's is what that thing costs at
//! that stop, in milliseconds of GPU time, with every other pass in the same state. The baseline is
//! read again after every variant, and a variant is compared with the two readings around it, so
//! drift over the minutes a sweep takes doesn't count. `--out NAME` writes `NAME.md` (tables to
//! read) and `NAME.json` (every number). `--shots` saves a picture of each stop,
//! `--variant-shots a,b` (or `all`) one under each of those variants too, as
//! `NAME-<stop>[-<variant>].png`: those are taken after the readings with the simulation paused,
//! next to `NAME-<stop>-paused.png`, so two of them differ only by what the switch does.
//! `--shots-only` takes the pictures without the readings (a software GPU's times say nothing).
//!
//! The cuts of `RenderSettings::trial_cuts` are on by default. `--old-way` takes them all off
//! for the sweep (and puts probe GI's rays back in software), so the baseline is the frame as it
//! was before them, each `cut_*` row reads
//! what that cut saves by itself and `cut_all` what the default frame saves. Without it the
//! baseline is the default frame: `old_way` is the whole before and after, the cuts that are
//! on change nothing, and those that are off (the trades) are added.
//!
//! `--skip a,b` leaves those switches out (one the game cannot take on a machine). The files are
//! written after every stop, so a sweep that dies keeps the stops it finished.
//!
//! `--variants a,b` runs only those (names as in the table; `none` for the baseline alone, which
//! is what a launch with an environment switch such as `TERRAIN_SHADER_OFF=1` wants, with
//! `--label` to name it). `--list` prints the variants. Stops are `showcase.goto` places; with
//! `--no-goto` the camera stays where it is (any game, one "stop" named `here`).
//!
//! Besides the GPU rows each stop notes the main thread's own time a frame (`frame.main_ms`,
//! with the profiler on or off) and keeps the soft bodies' numbers (`softbody.*`) in the JSON.
//! `--call a.method,b.method` calls those bridge methods once at each stop, after its baseline,
//! and keeps what they answer under the stop's `calls` (`--call softbody.list`: what every
//! cloth and rope costs there). `--stats prefix.,other.` keeps the game's numbers under those
//! prefixes (`engine.stats`), as means over half a second, under the stop's `stats` (as the
//! baseline has them) and under each row's `stats` (as they are while the row is on).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

/// One thing to switch: a JSON merge patch for a registered resource, and for more resources
/// where the thing is a setting of two modules together (`also`).
struct Variant {
    name: &'static str,
    resource: &'static str,
    /// Values to set.
    patch: Value,
    /// Numbers to multiply instead: (key, factor) of the resource's value at the time.
    times: &'static [(&'static str, f64)],
    /// More resources it patches, each with its values to set: set after `resource`, in this
    /// order, and put back before it, in the reverse one (`switch_on`, `switch_back`).
    also: Vec<(&'static str, Value)>,
    /// What it shows.
    note: &'static str,
}

impl Variant {
    /// Each resource it patches, with its values and the numbers it multiplies, in the order
    /// they are set: its own, then `also`'s (which multiply none).
    fn parts(
        &self,
    ) -> impl Iterator<Item = (&'static str, &Value, &'static [(&'static str, f64)])> {
        const NONE: &[(&str, f64)] = &[];
        std::iter::once((self.resource, &self.patch, self.times)).chain(
            self.also
                .iter()
                .map(|(resource, patch)| (*resource, patch, NONE)),
        )
    }
}

/// The patch to send to a resource as it stands, `current`: `patch` plus the `times` keys (their
/// values in `current` multiplied). The cuts' bits (`trial_cuts`, `trial_cuts_more`) are added
/// to those already on, so a row means the same against the default frame and against the old
/// way (`--old-way`).
fn patch_against(patch: &Value, times: &[(&str, f64)], current: &Value) -> Value {
    let mut out = patch.as_object().cloned().unwrap_or_default();
    for key in ["trial_cuts", "trial_cuts_more"] {
        let on = current.get(key).and_then(Value::as_u64).unwrap_or(0);
        if let Some(bits) = out.get(key).and_then(Value::as_u64) {
            out.insert(key.to_string(), json!(on | bits));
        }
    }
    for (key, factor) in times {
        let Some(now) = current.get(*key).and_then(Value::as_f64) else {
            continue;
        };
        let whole = current.get(*key).is_some_and(|v| v.is_u64() || v.is_i64());
        let scaled = now * factor;
        out.insert(
            (*key).to_string(),
            if whole {
                json!(scaled.round().max(0.0) as u64)
            } else {
                json!(scaled)
            },
        );
    }
    Value::Object(out)
}

fn variants() -> Vec<Variant> {
    let r = "RenderSettings";
    let v = |name, resource, patch, note| Variant {
        name,
        resource,
        patch,
        times: &[],
        also: Vec::new(),
        note,
    };
    // Multiplies numbers of the resource as it stands (thresholds a game or a preset set).
    let x = |name, resource, times, note| Variant {
        name,
        resource,
        patch: json!({}),
        times,
        also: Vec::new(),
        note,
    };
    vec![
        v(
            "half_scale",
            r,
            json!({"render_scale": 0.5, "temporal_upscaling": false}),
            "a quarter of the pixels: what stays is per vertex, per map or per frame",
        ),
        v(
            "no_volumetric_fog",
            r,
            json!({"volumetric_fog": false}),
            "froxel fog and light shafts",
        ),
        v(
            "no_probe_gi",
            r,
            json!({"probe_gi": false}),
            "probe tracing and the eight probe lookups a pixel",
        ),
        v(
            "software_rays",
            r,
            json!({"gi_hardware_rays": false}),
            "probe GI's and world reflections' rays walk the software hierarchy in a compute shader instead of the RT cores (as before 15 Nov; the same on a GPU without ray queries)",
        ),
        v(
            "no_ssgi",
            r,
            json!({"screen_space_gi": false}),
            "the bounce light pass",
        ),
        v(
            "no_ssr",
            r,
            json!({"screen_space_reflections": false, "world_reflections": false}),
            "screen-space and world reflections",
        ),
        v(
            "no_ao",
            r,
            json!({"ambient_occlusion": false}),
            "ambient occlusion",
        ),
        v(
            "no_contact_shadows",
            r,
            json!({"contact_shadow_length": 0.0}),
            "the 10-step sun march within 60 m",
        ),
        v(
            "no_distant_shadows",
            r,
            json!({"distant_shadows": 0.0}),
            "the 8-step sun march past the cascades",
        ),
        v(
            "shadow_taps_1",
            r,
            json!({"shadow_filter_taps": 1}),
            "the cascades' filter taps (the blocker search stays)",
        ),
        v(
            "no_shadow_insets",
            r,
            json!({"shadow_insets": 0}),
            "people's inset shadow maps",
        ),
        v(
            "short_shadows",
            r,
            json!({"shadow_distance": 1.0, "far_shadow_distance": 0.0}),
            "sun shadow maps: drawing them and reading them",
        ),
        v(
            "no_cloud_shadows",
            r,
            json!({"cloud_shadows": false}),
            "the cloud shadow map",
        ),
        v(
            "no_canopy",
            r,
            json!({"canopy": false}),
            "light through leaves (four canopy taps a pixel)",
        ),
        v(
            "no_sky_occlusion",
            r,
            json!({"sky_occlusion": false}),
            "the top-down sky map: its draw, the ring taps a pixel, the fog's taps",
        ),
        v(
            "no_taa",
            r,
            json!({"anti_aliasing": "None"}),
            "temporal anti-aliasing",
        ),
        v(
            "no_virtual_geometry",
            r,
            json!({"virtual_geometry": false}),
            "cluster rendering (meshes fall back to plain LODs)",
        ),
        v(
            "no_mesh_occlusion",
            r,
            json!({"mesh_occlusion_culling": false}),
            "HZB occlusion for instanced meshes",
        ),
        v(
            "freeze_skins",
            r,
            json!({"debug_skip": 524_288}),
            "skinned meshes are not posed (characters freeze): what GPU skinning costs in `compute meshes`",
        ),
        v(
            "skip_shadow_copies",
            r,
            json!({"debug_skip": 1_048_576}),
            "the shadow maps are put back to their still images only when those change (moving shadows leave trails): what putting back the tiles under moving casters costs in `shadows` and `sky occlusion` (with `--old-way`: what copying all six layers every frame cost)",
        ),
        v(
            "no_strands",
            r,
            json!({"debug_skip": 16_777_216}),
            "no hair or fur is drawn: what all of it costs (`compute strands`, `shadows: strands`, `before scene`)",
        ),
        v(
            "skip_strand_light",
            r,
            json!({"debug_skip": 4_194_304}),
            "hair and fur points take a flat color: what lighting them costs in `before scene`, apart from placing, sorting and drawing them",
        ),
        v(
            "skip_strand_raster",
            r,
            json!({"debug_skip": 33_554_432}),
            "the hair's tiles are not rasterized (the picture keeps old hair): what the raster costs in `before scene`, apart from placing, lighting and sorting the points",
        ),
        v(
            "strands_old_way",
            r,
            json!({"debug_skip": 8_388_608}),
            "hair and fur as before 17 Nov (a far coat draws a sixteenth of its strands however small it is; hidden strands are lit and sorted too): `before scene` costs this much MORE the old way",
        ),
        v(
            "strands_sparse",
            r,
            json!({"debug_skip": 67_108_864}),
            "a groom far away draws one strand a pixel of its square instead of four (coarser coats): what the finer number costs in `before scene`",
        ),
        v(
            "strands_near_count",
            r,
            json!({"debug_skip": 134_217_728}),
            "every groom draws the near rule's number of strands as before 17 Nov, hidden strands still left out: `before scene` costs this much MORE without the count by the pixels",
        ),
        v(
            "grass_old_way",
            r,
            json!({"debug_skip": 268_435_456}),
            "grass in the scene pass as before 17 Nov (its cut fronds lit at every layer before the depth test, every run reads whether it is the prepass's surface, and the light through a blade reads the sun's shadow maps a second time): `scene: grass` costs this much MORE the old way",
        ),
        v(
            "count_grass_runs",
            r,
            json!({"debug_skip": 262_144}),
            "a count, not a cut: how many times the blades' vertex and pixel programs run in the prepass and the scene pass (exact, counted by the GPU; in the json as grass_runs)",
        ),
        v(
            "count_grass_runs_old_way",
            r,
            json!({"debug_skip": 268_697_600}),
            "the count with grass_old_way on",
        ),
        v(
            "no_grass",
            r,
            json!({"grass_density": 0.0}),
            "grass and ground plants",
        ),
        v(
            "ray_scene_10k",
            r,
            json!({"ray_scene_stress": 10_000}),
            "a measurement, not a cut (the frame costs MORE): 10,000 more instances in the rays' scene, copies of those there spread 256 m each way, and the instances' structure built again every frame. Read `probe gi build` (the build), `probe gi` and `world reflections` (tracing through it), and in the json rows the CPU's part: `render.cpu.probe gi build`, `render.cpu.submit`, `render.gi.ms_instances`, `render.cpu_ms`",
        ),
        v(
            "ray_scene_20k",
            r,
            json!({"ray_scene_stress": 20_000}),
            "the same with 20,000",
        ),
        v(
            "ray_scene_40k",
            r,
            json!({"ray_scene_stress": 40_000}),
            "the same with 40,000",
        ),
        v(
            "ray_sun",
            r,
            json!({"ray_sun_shadows": true}),
            "a measurement, not a cut: one ray toward the sun from every pixel the prepass drew, against the rays' scene as it is today (pass `ray shadows`; nothing reads what the rays find)",
        ),
        v(
            "ray_sun_10k",
            r,
            json!({"ray_sun_shadows": true, "ray_scene_stress": 10_000}),
            "the sun's rays through the scene of ray_scene_10k (read `ray shadows` against that row)",
        ),
        v(
            "ray_sun_40k",
            r,
            json!({"ray_sun_shadows": true, "ray_scene_stress": 40_000}),
            "the sun's rays through the scene of ray_scene_40k",
        ),
        v(
            "count_ray_sun",
            r,
            json!({"ray_sun_shadows": true, "debug_skip": 262_144}),
            "a count: the sun's rays shot and those stopped (in the json state as render.gi.sun_rays and render.gi.sun_hits)",
        ),
        v(
            "count_ray_sun_40k",
            r,
            json!({"ray_sun_shadows": true, "ray_scene_stress": 40_000, "debug_skip": 262_144}),
            "the count through the scene of ray_scene_40k",
        ),
        v(
            "ray_world",
            r,
            json!({"trial_cuts_more": 16, "gi_hardware_rays": true}),
            "every ray of the frame traces one scene the features fill on the render thread (what holds still in one structure, what moves in a small one), in one pass after the prepass: read `ray scene build`, `ray inputs`, `probe gi`, `world reflections` and `probe gi copies` against the baseline's `probe gi build`, `probe gi` and `world reflections`; `--stats render.ray.,render.meshes.ray_` keeps what the scene holds while the row is on (under the row's `stats`)",
        ),
        v(
            "ray_world_sun",
            r,
            json!({"trial_cuts_more": 16, "gi_hardware_rays": true, "ray_sun_shadows": true}),
            "ray_world with the sun's measuring rays in its pass (`ray shadows`; read against ray_sun)",
        ),
        v(
            "count_ray_world_sun",
            r,
            json!({
                "trial_cuts_more": 16,
                "gi_hardware_rays": true,
                "ray_sun_shadows": true,
                "debug_skip": 262_144
            }),
            "the count of the sun's rays through the frame's ray scene (against count_ray_sun)",
        ),
        v(
            "ray_probes",
            r,
            json!({"trial_cuts_more": 32, "gi_hardware_rays": true}),
            "light probes four times as often on ray-tracing hardware (8,192 a frame within 1.6 times `gi_budget_ms`, 3.0 ms at Ultra) and probes stuck inside something asleep, on the probes' own scene: read `probe gi` against the baseline's; `--stats render.gi.` keeps `probes_updated`, `probes_budget_cut`, `probes_traced` and `probes_awake` (under the row's `stats`)",
        ),
        v(
            "ray_world_probes",
            r,
            json!({"trial_cuts_more": 48, "gi_hardware_rays": true}),
            "ray_world with ray_probes: four times the probes' rays through the frame's ray scene (read `probe gi` and `probe gi copies` against ray_world)",
        ),
        v(
            "ray_sun_trace",
            r,
            json!({"trial_cuts_more": 144, "gi_hardware_rays": true}),
            "ray_world with the sun's trace (in place of the measuring rays) joined with the shadow maps: a ray toward the sun's disc from each pixel within 80 m that faces the sun and does not sway, 64 m long, against the still structure (pass `ray shadows`, read against ray_world_sun), kept over the frames (pass `ray shadows filter`) and joined in the screen light pass with what the maps hold that the rays don't (read `ray shadows filter` and `screen light` against ray_sun_trace_only)",
        ),
        v(
            "ray_sun_trace_only",
            r,
            json!({"trial_cuts_more": 144, "gi_hardware_rays": true, "debug_skip": 1_073_741_824}),
            "ray_sun_trace without the filter and the join: the trace alone, which nothing reads (the light pass takes the shadow maps alone, as at the baseline)",
        ),
        v(
            "ray_sun_reach_160",
            r,
            json!({
                "trial_cuts_more": 144,
                "gi_hardware_rays": true,
                "ray_sun_reach": 160.0,
                "ray_caster_reach": 270.0
            }),
            "ray_sun_trace with the traced pixels out to 160 m in place of 80 m, and the casters the rays hold out to 270 m in place of 160 m (the join ends at 0.9 of the shadow distance where that is nearer)",
        ),
        v(
            "ray_sun_trace_leaves_solid",
            r,
            json!({"trial_cuts_more": 144, "gi_hardware_rays": true, "debug_skip": 536_870_912}),
            "ray_sun_trace with every cut-out card's candidate kept unread: `ray shadows` against ray_sun_trace is what reading the leaves' texture costs",
        ),
        v(
            "count_ray_sun_trace",
            r,
            json!({"trial_cuts_more": 144, "gi_hardware_rays": true, "debug_skip": 262_144}),
            "a count of the sun's trace (render.ray.sun_rays, sun_hits, and sun_missed and sun_false: where the shadow maps' still images, sure of a pixel, and a ray toward the sun's center disagree)",
        ),
        v(
            "ray_reflections",
            r,
            json!({"trial_cuts_more": 80, "gi_hardware_rays": true}),
            "ray_world with a reflection for every pixel up to the reflections' roughness limit, in place of screen-space reflections over world reflections at half size: each marches the screen first and takes last frame's picture of what it meets, else a ray through the frame's ray scene up to 384 m (shorter as the surface roughens), lit as a world reflection's where the camera does not see its hit (pass `ray reflections`, read against ray_world's `world reflections`), then blurred over each pixel's reflection lobe (pass `ray reflections resolve`, read against ray_world's `reflections`)",
        ),
        v(
            "ray_reflections_rays_only",
            r,
            json!({"trial_cuts_more": 80, "gi_hardware_rays": true, "debug_skip": 2_147_483_648u32}),
            "ray_reflections with every pixel's reflection a ray's, without the screen's march first: what tracing them all costs (`ray reflections` against ray_reflections)",
        ),
        v(
            "count_ray_reflections",
            r,
            json!({"trial_cuts_more": 80, "gi_hardware_rays": true, "debug_skip": 262_144}),
            "a count of every pixel's reflection (render.ray.reflect_pixels: pixels under the roughness limit; reflect_screen: answered by the screen's march; reflect_rays: traced; reflect_reused: rays whose hit the camera sees, which take last frame's picture)",
        ),
        v(
            "ray_round",
            r,
            json!({"trial_cuts_more": 240, "gi_hardware_rays": true}),
            "every built step of the ray-traced lighting round at once: ray_world with the probes four times as often, the sun's trace joined with the shadow maps and a reflection for every pixel (what step 10 is to turn on by default; read the whole frame against the baseline)",
        ),
        v(
            "ray_all",
            r,
            json!({"trial_cuts_more": 2032, "gi_hardware_rays": true}),
            "every bit of the ray-traced lighting round at once (`trial_cut_more::RAYS`): ray_round with the ground colour map, the water's own reflection ray and the lake mirror's picture traced in place of its extra view. What step 10 turns on by default if all seven join; read the whole frame against the baseline",
        ),
        v(
            "ray_ground_map",
            r,
            json!({"trial_cuts_more": 528, "gi_hardware_rays": true}),
            "ray_world with the ground colour map: the features that draw it (the terrain) draw their albedo seen from straight above into a 1024 x 1024 map 768 m across around the camera (0.75 m a texel) when a version or the map's centre changes, at most every 8th frame, and their ground hits take the map's colour in place of the air's one ground colour (pass `ray ground map`, 0 in the frames that do not draw it; `--stats render.ray.ground_` keeps ground_map, ground_draws, ground_drawers, ground_mb; without a feature that draws it the row is ray_world)",
        ),
        v(
            "ray_ground_map_every_frame",
            r,
            json!({"trial_cuts_more": 528, "gi_hardware_rays": true, "ray_ground_every_frame": true}),
            "ray_ground_map drawn every frame (`ray_ground_every_frame`, a measurement): pass `ray ground map` is one draw's cost, the features' part and the map's levels; the picture is ray_ground_map's",
        ),
        v(
            "ray_mirror_pictures",
            r,
            json!({"trial_cuts_more": 1040, "gi_hardware_rays": true}),
            "ray_world with the lake mirror's picture traced in the frame's ray pass in place of the extra view that draws it: each texel is the reflection, at roughness 0, of where its ray from the mirrored eye meets the plane (passes `ray mirror pictures`, and `ray mirror levels` with the picture's own history; render.extra_views 0; render.ray.mirror_pictures, mirror_texels, mirror_mb). Its saves against ray_world at the lake are what the drawn mirror cost (its view, copy and levels: `render.extra_views_gpu_ms`, which the GPU column counts) less what the traced picture costs",
        ),
        v(
            "count_ray_mirror_pictures",
            r,
            json!({"trial_cuts_more": 1040, "gi_hardware_rays": true, "debug_skip": 262_144}),
            "a count of the traced mirror pictures' texels (render.ray.mirror_screen: answered by the screen's march; mirror_rays: traced; mirror_reused: rays whose hit the camera sees, which take last frame's picture)",
        ),
        v(
            "water_no_mirror",
            "WaterSettings",
            json!({"mirror_reflections": false}),
            "a control, not a cut: no lake mirror, the water's screen-space reflections alone (on every water pixel, so the main view gets dearer); its saves are what the drawn mirror costs, its view, copy and levels (`render.extra_views_gpu_ms`, which the GPU column counts; extra_views_ms its CPU), less the water's own reflections in its place",
        ),
        v(
            "ray_water",
            r,
            json!({"trial_cuts_more": 272, "gi_hardware_rays": true}),
            "ray_world with the water's own reflection ray: a calm body's pixel (waves of 0.25 m or less, the lake mirror's test) that the mirror doesn't fully answer traces the frame's ray scene along its reflection where the screen's march finds nothing or fades at the screen's edge, and lights its hit as every reflection's hits are lit (two queries and the hit's light a traced pixel; water.ray 1 while the bodies draw with it, water.ray_bodies the calm bodies). Its cost is in the water's draw (pass `transparent`, read against ray_world's) and in render.cpu.submit (one more walk of the scene's instances where the bodies bind it); where every water pixel is the mirror's (a still lake in full view), little traces",
        ),
        Variant {
            name: "ray_water_no_mirror",
            resource: r,
            patch: json!({"trial_cuts_more": 272, "gi_hardware_rays": true}),
            times: &[],
            also: vec![("WaterSettings", json!({"mirror_reflections": false}))],
            note: "ray_water without the lake mirror (with water_no_mirror's WaterSettings): the lake's reflections from the screen's march and the frame's rays alone, render.extra_views 0. Against water_no_mirror: what the rays add where the march alone stood; against ray_water: what the drawn mirror costs (`render.extra_views_gpu_ms`, which the GPU column counts) and what it shows that the rays don't (its picture is blurred by roughness, a single ray isn't)",
        },
        v(
            "check_ray_probe",
            r,
            json!({"trial_cuts_more": 48, "gi_hardware_rays": true, "ray_probe_check": true}),
            "a check, not a cut: ray_world_probes with the probe nearest the camera against a brute-force reference of 4,096 rays a direction a frame through the frame's ray scene (`--stats render.gi.check_`: check_probe_*, check_ref_* and check_ratio_* for the six axis directions, means over the same frames; check_worst the farthest ratio from 1 over the directions whose reference holds a tenth of the brightest's; check_spread_* and check_flicker the probe's flicker from frame to frame; pass `probe check` is the check's own cost)",
        ),
        v(
            "check_ray_probe_world",
            r,
            json!({"trial_cuts_more": 16, "gi_hardware_rays": true, "ray_probe_check": true}),
            "check_ray_probe with the probes' own count of rays (ray_world without ray_probes)",
        ),
        v(
            "skip_lighting",
            r,
            json!({"debug_skip": 128}),
            "all of shade_surface: what is left of the scene pass is materials and fog",
        ),
        v(
            "skip_sun_shadow_maps",
            r,
            json!({"debug_skip": 1}),
            "reading the sun's shadow maps in every pixel (they are still drawn)",
        ),
        v(
            "skip_capsules",
            r,
            json!({"debug_skip": 2}),
            "people's capsule shadows",
        ),
        v(
            "skip_sky_volumes",
            r,
            json!({"debug_skip": 4}),
            "the indoor volumes' loops in every pixel",
        ),
        v(
            "skip_probe_lookup",
            r,
            json!({"debug_skip": 8}),
            "the eight probes a pixel reads (tracing still runs)",
        ),
        v(
            "skip_specular_sky",
            r,
            json!({"debug_skip": 16}),
            "the reflection's 18-tap march through the sky map",
        ),
        v(
            "skip_point_lights",
            r,
            json!({"debug_skip": 32}),
            "the point light loop in every pixel",
        ),
        v(
            "skip_surface_fog",
            r,
            json!({"debug_skip": 64}),
            "fog and aerial perspective on surfaces",
        ),
        v(
            "skip_sky_map_taps",
            r,
            json!({"debug_skip": 8192}),
            "the sky map's ring taps in pixels and fog (the map is still drawn)",
        ),
        v(
            "skip_terrain_textures",
            r,
            json!({"debug_skip": 256}),
            "the ground's material textures",
        ),
        v(
            "skip_terrain_noise",
            r,
            json!({"debug_skip": 512}),
            "the ground's fractal noises",
        ),
        v(
            "skip_terrain",
            r,
            json!({"debug_skip": 1024}),
            "the ground's whole pixel shader (its triangles stay)",
        ),
        v(
            "skip_mesh_scene",
            r,
            json!({"debug_skip": 2048}),
            "opaque meshes' whole scene pixel shader (their triangles stay)",
        ),
        v(
            "skip_mesh_prepass_material",
            r,
            json!({"debug_skip": 4096}),
            "opaque meshes' material in the prepass",
        ),
        v(
            "no_clouds",
            "CloudSettings",
            json!({"enabled": false}),
            "volumetric clouds: their march, their shadow map, their part of the sky",
        ),
        v(
            "clouds_half_steps",
            "CloudSettings",
            json!({"steps": 36, "light_steps": 3}),
            "half the cloud march's samples",
        ),
        v(
            "clouds_quarter_res",
            "CloudSettings",
            json!({"resolution_scale": 0.25}),
            "the cloud image at a quarter of the screen's width",
        ),
        v(
            "no_bloom",
            r,
            json!({"bloom_intensity": 0.0, "sun_glare": 0.0}),
            "bloom and glare",
        ),
        // Cuts being tried: the same picture for less (`RenderSettings::trial_cuts`, pacing,
        // the cloud march's share of the image). Take their pictures with --variant-shots.
        v(
            "cut_sky_ring_taps",
            r,
            json!({"trial_cuts": 1}),
            "sky cover read with four turning taps a ring, not eight fixed ones",
        ),
        v(
            "cut_terrain_aniso_8",
            r,
            json!({"trial_cuts": 2}),
            "ground textures with at most 8 anisotropic probes, not 16",
        ),
        v(
            "cut_terrain_aniso_4",
            r,
            json!({"trial_cuts": 4}),
            "ground textures with at most 4 anisotropic probes",
        ),
        v(
            "cut_terrain_copy_aniso",
            r,
            json!({"trial_cuts": 8}),
            "the ground textures' larger copies with at most 4 anisotropic probes",
        ),
        v(
            "cut_terrain_fade_aniso",
            r,
            json!({"trial_cuts": 16}),
            "a ground texture's probes follow how much of it still shows (16 down to 2)",
        ),
        v(
            "cut_terrain_copy_single",
            r,
            json!({"trial_cuts": 32}),
            "the 4x distance copy of meadows, fields and roads with one lookup, not the anti-tiling pair",
        ),
        v(
            "cut_terrain_rock_single",
            r,
            json!({"trial_cuts": 1_048_576}),
            "rock's 8x and 64x copies with one lookup each, not the pair: far mountainsides read smoother, so this one is a trade to judge from its picture",
        ),
        v(
            "cut_terrain_copies",
            r,
            json!({"trial_cuts": 56}),
            "the three ground texture cuts together",
        ),
        v(
            "cut_shadow_umbra",
            r,
            json!({"trial_cuts": 64}),
            "deep shadow skips the cascades' filter taps",
        ),
        v(
            "cut_sun_march_steps",
            r,
            json!({"trial_cuts": 128}),
            "contact shadows with 6 steps (not 10), the far sun march with 5 (not 8)",
        ),
        v(
            "cut_specular_sky_taps",
            r,
            json!({"trial_cuts": 256}),
            "the reflection's sky march with 9 taps, not 18",
        ),
        v(
            "cut_lighting",
            r,
            json!({"trial_cuts": 449}),
            "the four lighting cuts together (sky rings, deep shadow, sun marches, reflection march)",
        ),
        v(
            "cut_screen_light",
            r,
            json!({"trial_cuts": 512}),
            "shadows, sky cover and probe light once per pixel in a screen pass, not in every shader run",
        ),
        v(
            "cut_screen_light_thin",
            r,
            json!({"trial_cuts": 2_097_664}),
            "the screen light pass, with leaves and other thin sheets reading the sun's shadows themselves (their exact picture; the pass alone moves single pixels on leaves)",
        ),
        v(
            "cut_bounce_half",
            r,
            json!({"trial_cuts": 4_194_304}),
            "bounce light traced and blurred for every fourth pixel with twice the rays, then spread to every pixel by depth and normal (the frame stays full size; a trade to judge from its picture)",
        ),
        v(
            "cut_reflections_alternate",
            r,
            json!({"trial_cuts": 8_388_608}),
            "world reflections trace every other ray a frame, the rest carried over from the frame before by the screen motion (the same picture at rest; to judge in motion)",
        ),
        v(
            "cut_aerial_stepwise",
            r,
            json!({"trial_cuts_more": 1}),
            "the haze table's atmosphere carried on from slice to slice: 64 steps a column, not 512",
        ),
        v(
            "cut_ground_still",
            r,
            json!({"trial_cuts_more": 2}),
            "the ground stays in the shadow maps' still images (four cascades and the far map); only the patches an image doesn't hold are drawn on top each frame",
        ),
        v(
            "cut_shadow_tiles",
            r,
            json!({"trial_cuts_more": 4}),
            "a shadow map is put back to its still image only in the tiles where something moved the frame before; the six layers were copied whole every frame (335 MB of depth at 4096)",
        ),
        v(
            "cut_fog_reach",
            r,
            json!({"trial_cuts_more": 8}),
            "the fog volume lights only the cells in front of what the picture shows (all 7.4 M of them every frame at 2560x1440 before)",
        ),
        // Other modules' own switches, each a trade its owner left off until it is judged from
        // pictures.
        v(
            "water_vertex_sky_and_fog",
            "WaterSettings",
            json!({"vertex_sky_and_fog": true}),
            "lakes and seas take the open sky's share and the fog from their vertices, not every pixel (lake stop)",
        ),
        v(
            "smoke_half_res",
            "SmokeSettings",
            json!({"half_res": true}),
            "smoke marched at half the screen's resolution in each direction and laid over the scene (field, fortress)",
        ),
        v(
            "cut_tiled_lamps",
            r,
            json!({"trial_cuts": 1024}),
            "each pixel goes through the lamps that reach its 16-pixel tile, not every lamp in view",
        ),
        v(
            "cut_screen_fog",
            r,
            json!({"trial_cuts": 2048}),
            "fog and haze once per pixel in the screen pass, not in every shader run",
        ),
        v(
            "cut_ground_lines",
            r,
            json!({"trial_cuts": 4096}),
            "ground seen edge on drawn without every second or fourth row of triangles across the view",
        ),
        v(
            "cut_ground_lines_wide",
            r,
            json!({"trial_cuts": 8192}),
            "the same, and the triangles left may look half again as long across the view as they are wide",
        ),
        v(
            "cut_ground_once",
            r,
            json!({"trial_cuts": 16384}),
            "the ground's color once a pixel, not once for every triangle that touches the pixel's 2x2 block (a pass \"before scene\" writes what the vertices carry)",
        ),
        v(
            "cut_meshes_once",
            r,
            json!({"trial_cuts": 32768}),
            "objects (trees, buildings, rocks) work out their material in one pass, not in the depth pass and again in the color pass, and are lit once a pixel; their vertices are drawn once, not twice",
        ),
        v(
            "show_meshes_once",
            r,
            json!({"trial_cuts": 32768, "debug_skip": 131072}),
            "a picture, not a cut: the pixels of objects lit once from their stored material in magenta",
        ),
        v(
            "cut_meshes_early_z",
            r,
            json!({"trial_cuts": 65536}),
            "objects' color program runs after the depth test, not before it: hidden layers are rejected without shading them (their programs can write a texture request, which makes the GPU run them first unless told otherwise)",
        ),
        v(
            "cut_meshes_depth_first",
            r,
            json!({"trial_cuts": 163840}),
            "cut_meshes_once with objects' depth drawn first, so their material is worked out for the layer in view only, not for every layer drawn",
        ),
        v(
            "cut_once_depth_first_all",
            r,
            json!({"trial_cuts": 180224}),
            "cut_ground_once, cut_meshes_once and the depth first together",
        ),
        v(
            "cut_meshes_holes_first",
            r,
            json!({"trial_cuts": 425984}),
            "cut_meshes_depth_first with leaves and other cut-out materials in the depth step too: a single texture read finds their holes, and their whole material is worked out for the layer in view only",
        ),
        v(
            "cut_once_holes_first_all",
            r,
            json!({"trial_cuts": 442368}),
            "cut_ground_once, cut_meshes_once, the depth first and the holes first together",
        ),
        v(
            "count_mesh_runs_holes_first",
            r,
            json!({"trial_cuts": 425984, "debug_skip": 262144}),
            "the count with cut_meshes_holes_first on",
        ),
        v(
            "cut_meshes_near_first",
            r,
            json!({"trial_cuts": 524288}),
            "objects are drawn in bands of distance, nearest first, so those behind are turned away by the depth test and their material is not worked out (order only: nothing else changes on the GPU; more batches for the CPU)",
        ),
        v(
            "cut_meshes_once_near_first",
            r,
            json!({"trial_cuts": 557056}),
            "cut_meshes_once with the nearest-first order: fewer hidden layers without drawing the vertices twice",
        ),
        v(
            "count_mesh_runs_near_first",
            r,
            json!({"trial_cuts": 524288, "debug_skip": 262144}),
            "the count with cut_meshes_near_first on",
        ),
        v(
            "count_mesh_runs_once_near_first",
            r,
            json!({"trial_cuts": 557056, "debug_skip": 262144}),
            "the count with cut_meshes_once_near_first on",
        ),
        v(
            "count_mesh_runs_early_z",
            r,
            json!({"trial_cuts": 65536, "debug_skip": 262144}),
            "the count with cut_meshes_early_z on",
        ),
        v(
            "count_mesh_runs_depth_first",
            r,
            json!({"trial_cuts": 163840, "debug_skip": 262144}),
            "the count with cut_meshes_depth_first on",
        ),
        v(
            "count_mesh_runs",
            r,
            json!({"debug_skip": 262144}),
            "a count, not a cut: how many times objects' vertex and pixel programs run in the prepass and the scene pass (exact, counted by the GPU; in the json as mesh_runs)",
        ),
        v(
            "count_mesh_runs_once",
            r,
            json!({"trial_cuts": 32768, "debug_skip": 262144}),
            "the same count with cut_meshes_once on",
        ),
        v(
            "cut_once_all",
            r,
            json!({"trial_cuts": 49152}),
            "cut_ground_once and cut_meshes_once together",
        ),
        v(
            "show_ground_once",
            r,
            json!({"trial_cuts": 16384, "debug_skip": 65536}),
            "a picture, not a cut: the ground colored once a pixel in magenta, the rest of the ground in green",
        ),
        v(
            "show_screen_light",
            r,
            json!({"trial_cuts": 512, "debug_skip": 16384}),
            "a picture, not a cut: the pixels that take their light from the screen pass lose the sun",
        ),
        v(
            "triangle_view",
            r,
            json!({
                "debug_skip": 32768, "tonemapper": "None", "bloom_intensity": 0.0,
                "sun_glare": 0.0, "saturation": 1.0, "contrast": 1.0, "vignette": 0.0,
                "film_grain": 0.0, "white_balance": 0.0, "anti_aliasing": "None",
                "volumetric_fog": false, "screen_space_gi": false,
                "screen_space_reflections": false, "world_reflections": false,
                "ambient_occlusion": false, "sharpness": 0.0, "night_desaturation": 0.0
            }),
            "a picture for counting shader runs per pixel: only in a build with the triangle-view patch (engine-perf/triangle-view)",
        ),
        v(
            "cut_lighting_all",
            r,
            json!({"trial_cuts": 4033}),
            "the screen pass with fog, lamp tiles and the four lighting cuts together",
        ),
        v(
            "cut_all",
            r,
            // 4033 lighting, 56 ground texture copies, 2 ground aniso 8, 4096 ground lines,
            // 16384 ground once, 32768..524288 the meshes' five. Not the three that change the
            // picture: the deep-shadow shortcut (64: dappled light's outer rim trimmed), a
            // reflection's halved cover taps (256: the plaza fountain's water 4 of 255
            // brighter) and rock's single lookup (1048576).
            // With 2097152: thin sheets read the sun's shadows themselves under the pass.
            // `trial_cuts_more` 1: the haze table in one sweep; 2: the ground in the shadow
            // maps' still images; 4: the maps put back by tile; 8: the fog volume's cells in
            // reach only.
            json!({"trial_cuts": 3_137_211, "trial_cuts_more": 15, "shadow_refresh_secs": 0.25}),
            "every same-picture cut in RenderSettings at once (lighting but for the deep-shadow shortcut and the reflection taps, ground textures but for rock's single lookup, ground lines, ground and objects once a pixel, depth first, holes first, nearest first, far shadow refresh): the frame if all of them pass",
        ),
        Variant {
            name: "old_way",
            resource: r,
            patch: json!({"gi_hardware_rays": false}),
            times: &[
                ("trial_cuts", 0.0),
                ("trial_cuts_more", 0.0),
                ("shadow_refresh_secs", 0.0),
                ("pacing_fps", 0.0),
            ],
            also: Vec::new(),
            note: "the frame as it was drawn before 11 Nov 2026 (no cut on, no pacing, the rays in software): against the default frame, the whole before and after",
        },
        v(
            "cut_all_no_screen_pass",
            r,
            // `cut_all` without 512, 2048 and 2097152.
            json!({"trial_cuts": 1_037_499, "trial_cuts_more": 15, "shadow_refresh_secs": 0.25}),
            "cut_all without the screen pass (light and fog once per pixel): with ground and objects already shaded once a pixel, does the pass still pay?",
        ),
        v(
            "cut_shadow_refresh",
            r,
            json!({"shadow_refresh_secs": 0.25}),
            "far shadow maps redrawn for changed casters every 0.25 to 1 s, not every frame",
        ),
        v(
            "cut_pacing_30",
            r,
            json!({"pacing_fps": 30.0}),
            "work spread over frames paced for 30 a second: less a frame above that",
        ),
        v(
            "cut_clouds_ninth",
            "CloudSettings",
            json!({"temporal_side": 3}),
            "a ninth of the cloud image marched a frame, not a quarter",
        ),
        v(
            "cut_clouds_sixteenth",
            "CloudSettings",
            json!({"temporal_side": 4}),
            "a sixteenth of the cloud image marched a frame",
        ),
        // What the Ultra preset adds over High, one factor at a time, and all at once.
        v(
            "high_view_distance",
            "Scalability",
            json!({"view_distance": 1.0}),
            "High's detail thresholds (Ultra keeps levels of detail 1.5x farther)",
        ),
        v(
            "high_shadows",
            "Scalability",
            json!({"shadows": 1.0}),
            "High's shadow maps and shadow distance",
        ),
        v(
            "high_density",
            "Scalability",
            json!({"density": 1.0}),
            "High's grass density",
        ),
        v(
            "high_effects",
            "Scalability",
            json!({"effects": 1.0}),
            "High's effects budget (probe tracing time, point lights)",
        ),
        v(
            "high_preset",
            "Scalability",
            json!({"view_distance": 1.0, "shadows": 1.0, "density": 1.0, "effects": 1.0}),
            "the High preset: the whole gap between Ultra and High",
        ),
        // Triangle density: thresholds in pixels or texels, doubled.
        x(
            "cluster_error_x2",
            r,
            &[("cluster_error_px", 2.0)],
            "virtual geometry at twice the pixel error: fewer, larger triangles",
        ),
        x(
            "mesh_lod_x2",
            r,
            &[("mesh_lod_pixels", 2.0)],
            "ordinary meshes switch to coarser levels at twice the size",
        ),
        x(
            "min_object_x2",
            r,
            &[("min_object_pixels", 2.0)],
            "objects under two pixels (not one) are left out",
        ),
        x(
            "shadow_detail_x2",
            r,
            &[
                ("cluster_shadow_error_texels", 2.0),
                ("shadow_caster_min_texels", 2.0),
            ],
            "shadow casters at twice the texel error, small ones left out sooner",
        ),
        x(
            "shadow_map_half",
            r,
            &[("shadow_map_size", 0.5)],
            "shadow maps at half the size",
        ),
        x(
            "gi_budget_half",
            r,
            &[("gi_budget_ms", 0.5)],
            "half the GPU time a frame for probe tracing",
        ),
        x(
            "gi_rays_half",
            r,
            &[("gi_rays_per_probe", 0.5)],
            "half the rays a probe",
        ),
        v(
            "fog_medium",
            r,
            json!({"volumetric_fog_quality": "Medium"}),
            "the fog grid with 64 depth slices, not 128",
        ),
        x(
            "environment_slower",
            r,
            &[("environment_interval", 2.0)],
            "the sky's environment map half as often",
        ),
        x(
            "terrain_coarser",
            "TerrainSettings",
            &[("split_factor", 0.7)],
            "ground patches split at 0.7 of the distance: about half the ground triangles",
        ),
        // The patch budget raises the detail again when the split distance alone would draw
        // fewer patches than it allows, so `terrain_coarser` by itself moves the ground's
        // triangles by 2% at the plaza. With half the budget too: 48 to 72% of them (counted
        // 10 Nov at the plaza, the overview and the lake).
        x(
            "terrain_coarser_ground",
            "TerrainSettings",
            &[("split_factor", 0.7), ("patch_budget", 0.5)],
            "ground patches split at 0.7 of the distance with half the patch budget",
        ),
        x(
            "terrain_coarser_ground_more",
            "TerrainSettings",
            &[("split_factor", 0.5), ("patch_budget", 0.5)],
            "ground patches split at half the distance with half the patch budget",
        ),
        x(
            "grass_half",
            r,
            &[("grass_density", 0.5)],
            "half the grass blades",
        ),
    ]
}

struct Bridge {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl Bridge {
    fn connect(addr: &str, wait: Duration) -> Result<Self, String> {
        let start = Instant::now();
        loop {
            match TcpStream::connect(addr) {
                Ok(stream) => {
                    stream.set_nodelay(true).ok();
                    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();
                    let writer = stream.try_clone().map_err(|e| e.to_string())?;
                    return Ok(Self {
                        reader: BufReader::new(stream),
                        writer,
                        next_id: 1,
                    });
                }
                Err(e) if start.elapsed() > wait => {
                    return Err(format!("no game bridge at {addr}: {e}"));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.writer, "{request}").map_err(|e| format!("{method}: {e}"))?;
        loop {
            let mut line = String::new();
            let n = self
                .reader
                .read_line(&mut line)
                .map_err(|e| format!("{method}: {e}"))?;
            if n == 0 {
                return Err(format!("{method}: the game closed the connection"));
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!("{method}: {error}"));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

/// The values at the keys of `patch` in `current` (what to patch back afterwards).
fn restore_of(current: &Value, patch: &Value) -> Value {
    let mut out = Map::new();
    if let Some(patch) = patch.as_object() {
        for key in patch.keys() {
            out.insert(
                key.clone(),
                current.get(key).cloned().unwrap_or(Value::Null),
            );
        }
    }
    Value::Object(out)
}

/// What a row switches: the game's registered resources, read whole and set by a JSON merge
/// patch (the bridge's `resource.get` and `resource.set`; a test's stand-in).
trait Resources {
    fn get(&mut self, name: &str) -> Result<Value, String>;
    fn set(&mut self, name: &str, value: &Value) -> Result<(), String>;
}

impl Resources for Bridge {
    fn get(&mut self, name: &str) -> Result<Value, String> {
        self.call("resource.get", json!({"name": name}))
    }

    fn set(&mut self, name: &str, value: &Value) -> Result<(), String> {
        self.call("resource.set", json!({"name": name, "value": value}))
            .map(|_| ())
    }
}

/// A row switched on: what each of its resources was sent, and what puts it back.
struct SwitchedOn {
    /// The patch its own resource was sent ([`patch_against`]).
    patch: Value,
    /// The patches its other resources were sent, by name (`Variant::also`; empty without).
    also: Map<String, Value>,
    /// Each resource with what puts it back, in the order they were set.
    back: Vec<(&'static str, Value)>,
}

/// Switches row `v` on: each of its resources ([`Variant::parts`]) read and patched in turn, as
/// it stands then. A resource that can't be read or set skips the row: those set before it are
/// put back first, and the error names it.
fn switch_on(res: &mut impl Resources, v: &Variant) -> Result<SwitchedOn, String> {
    let mut on = SwitchedOn {
        patch: Value::Null,
        also: Map::new(),
        back: Vec::new(),
    };
    for (i, (resource, patch, times)) in v.parts().enumerate() {
        let set = match res.get(resource) {
            Ok(current) => {
                let patch = patch_against(patch, times, &current);
                let back = restore_of(&current, &patch);
                res.set(resource, &patch).map(|()| (patch, back))
            }
            Err(_) => Err(format!("no {resource}")),
        };
        match set {
            Ok((patch, back)) => {
                if i == 0 {
                    on.patch = patch;
                } else {
                    on.also.insert(resource.to_string(), patch);
                }
                on.back.push((resource, back));
            }
            Err(e) => {
                switch_back(res, &on)?;
                return Err(e);
            }
        }
    }
    Ok(on)
}

/// Puts back what [`switch_on`] set, the last resource set first.
fn switch_back(res: &mut impl Resources, on: &SwitchedOn) -> Result<(), String> {
    for (resource, back) in on.back.iter().rev() {
        res.set(resource, back)?;
    }
    Ok(())
}

#[derive(Default, Clone)]
struct Sample {
    frames: u32,
    frame_ms: f64,
    /// Sum over frames of each metric.
    sums: BTreeMap<String, f64>,
}

impl Sample {
    fn mean(&self, key: &str) -> f64 {
        self.sums.get(key).copied().unwrap_or(0.0) / f64::from(self.frames.max(1))
    }

    fn frame_ms(&self) -> f64 {
        self.frame_ms / f64::from(self.frames.max(1))
    }

    /// The frame's GPU time: the main view's (the profiler's own total where it reports one,
    /// else its passes summed; a split pass's parts, "scene: meshes", are inside "scene") and
    /// the extra views' drawn before it (`render.extra_views_gpu_ms`, 0 without: the lake
    /// mirror's view, its copy and levels included, which `ray_mirror_pictures` and
    /// `water_no_mirror` take away).
    fn gpu_ms(&self) -> f64 {
        let extra = self.mean("render.extra_views_gpu_ms");
        if self.sums.contains_key("render.gpu_ms") {
            return self.mean("render.gpu_ms") + extra;
        }
        self.sums
            .keys()
            .filter(|k| k.starts_with("render.gpu.") && !k.contains(": "))
            .map(|k| self.mean(k))
            .sum::<f64>()
            + extra
    }

    /// The mean of two readings, frame for frame weight.
    fn mean_of(a: &Sample, b: &Sample) -> Sample {
        let mut out = a.clone();
        out.frames += b.frames;
        out.frame_ms += b.frame_ms;
        for (key, sum) in &b.sums {
            *out.sums.entry(key.clone()).or_default() += sum;
        }
        out
    }

    fn to_json(&self) -> Value {
        let rows: Map<String, Value> = self
            .sums
            .keys()
            .map(|k| (k.clone(), json!(round3(self.mean(k)))))
            .collect();
        json!({
            "frames": self.frames,
            "frame_ms": round3(self.frame_ms()),
            "gpu_ms": round3(self.gpu_ms()),
            "rows": rows,
        })
    }
}

/// One switch's reading and the scene as it was around it.
struct Row {
    name: &'static str,
    with: Sample,
    around: Sample,
    triangles: Option<u64>,
}

impl Row {
    fn saves(&self) -> f64 {
        self.around.gpu_ms() - self.with.gpu_ms()
    }
}

/// Save a picture of the view (a failure is reported, not fatal) and give it time to be written.
fn shot(bridge: &mut Bridge, path: &str) -> Result<(), String> {
    if let Err(e) = bridge.call("render.screenshot", json!({"path": path})) {
        eprintln!("screenshot: {e}");
    }
    wait(bridge, 1.0)
}

/// What the rows are read against that is not a pass: whether the device has ray queries, probe
/// GI's numbers (`render.gi.*`: the path its rays trace on, its structures, what was lost) and
/// the GPU memory in use and reserved.
fn gi_and_memory(bridge: &mut Bridge) -> Option<Value> {
    let stats = bridge
        .call("engine.stats", json!({"prefix": "render."}))
        .ok()?;
    let kept: Map<String, Value> = stats
        .get("metrics")?
        .as_object()?
        .iter()
        .filter(|(key, _)| {
            key.starts_with("render.gi.")
                || matches!(
                    key.as_str(),
                    "render.ray_queries"
                        | "render.backend"
                        | "render.gpu_memory_mb"
                        | "render.gpu_reserved_mb"
                )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Some(Value::Object(kept))
}

/// The game's numbers under `prefix` (`engine.stats`), each as its mean over the distinct
/// frames of `secs` seconds, and how many frames that was. `None` when the game does not
/// answer or holds none.
fn mean_numbers(
    bridge: &mut Bridge,
    prefix: &str,
    secs: f32,
) -> Option<(u32, BTreeMap<String, f64>)> {
    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    let mut frames = 0u32;
    let start = Instant::now();
    let mut last_frame = u64::MAX;
    while start.elapsed().as_secs_f32() < secs || frames == 0 {
        let stats = bridge
            .call("engine.stats", json!({"prefix": prefix}))
            .ok()?;
        let frame = stats.get("frame").and_then(Value::as_u64).unwrap_or(0);
        if frame == last_frame {
            if start.elapsed().as_secs_f32() > secs.max(1.0) * 20.0 {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        last_frame = frame;
        for (key, value) in stats.get("metrics")?.as_object()? {
            if let Some(value) = value.as_f64() {
                *sums.entry(key.clone()).or_default() += value;
            }
        }
        frames += 1;
    }
    if sums.is_empty() {
        return None;
    }
    for sum in sums.values_mut() {
        *sum = round3(*sum / f64::from(frames));
    }
    Some((frames, sums))
}

/// The game's numbers under each of `prefixes` (`--stats`), as means over half a second, by
/// prefix. `None` when none was asked for or the game holds none; `at` names the stop or row
/// in the note about a prefix without numbers.
fn asked_stats(bridge: &mut Bridge, prefixes: &[String], at: &str) -> Option<Value> {
    let mut asked = Map::new();
    for prefix in prefixes {
        if let Some((frames, means)) = mean_numbers(bridge, prefix, 0.5) {
            asked.insert(prefix.clone(), json!({"frames": frames, "means": means}));
        } else {
            eprintln!("{at}: no numbers under {prefix}");
        }
    }
    (!asked.is_empty()).then_some(Value::Object(asked))
}

/// The main thread's own time a frame at a stop, as the game reports it with the profiler on
/// or off (`frame.main_ms`: its work through the schedules, the render system's share
/// included, without its wait for the render thread; `frame.main_fixed_ms`: the fixed steps'
/// part of it; `frame.main_wait_ms`: the wait). Means over `secs` seconds.
fn main_thread(bridge: &mut Bridge, secs: f32) -> Option<Value> {
    let (frames, means) = mean_numbers(bridge, "frame.", secs)?;
    Some(json!({
        "frames": frames,
        "main_ms": means.get("frame.main_ms")?,
        "fixed_ms": means.get("frame.main_fixed_ms")?,
        "wait_ms": means.get("frame.main_wait_ms")?,
    }))
}

/// The game's numbers under `prefix` as they stand (`softbody.`: what the cloth solved beside
/// the frame waits and holds), for the stop's JSON. `None` where there are none.
fn numbers(bridge: &mut Bridge, prefix: &str) -> Option<Value> {
    let stats = bridge
        .call("engine.stats", json!({"prefix": prefix}))
        .ok()?;
    let metrics = stats.get("metrics")?.as_object()?;
    (!metrics.is_empty()).then(|| Value::Object(metrics.clone()))
}

/// Triangles drawn in the last frame.
fn triangles(bridge: &mut Bridge) -> Option<u64> {
    bridge.call("render.stats", json!({})).ok()?["stats"]["triangles"].as_u64()
}

/// A feature's exact shader run counts of the last frame read back
/// (`shade_skip::COUNT_MESH_RUNS`; `key`: `mesh_runs` or `grass_runs`), where one reports them.
fn runs_of(bridge: &mut Bridge, key: &str) -> Option<Value> {
    let at = bridge.call("render.stats", json!({})).ok()?;
    at["renderer"]["features"]
        .as_array()?
        .iter()
        .map(|f| &f["state"][key])
        .find(|runs| runs.is_object())
        .cloned()
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// What a reading keeps besides the passes' GPU times: the CPU time of recording the rays'
/// structures' builds and the sun's rays, of placing the instances and of the gather as a
/// whole, of finishing and submitting the frame's commands (wgpu walks a top-level structure's
/// instances again there), the render's CPU time in all, the probes planned and what a
/// thousand of their rays cost, and what their update did (the budget's cut, the probes that
/// traced and those awake after, of those counted).
const RAY_ROWS: [&str; 14] = [
    "render.cpu.probe gi build",
    "render.cpu.ray shadows",
    "render.cpu.finish commands",
    "render.cpu.wait for the finishing threads",
    "render.cpu.submit",
    "render.cpu_ms",
    "render.gi.ms_instances",
    "render.gi.extract_ms",
    "render.gi.probes_updated",
    "render.gi.gpu_ms_per_kray",
    "render.gi.probes_budget_cut",
    "render.gi.probes_counted",
    "render.gi.probes_traced",
    "render.gi.probes_awake",
];

/// Read the render rows of distinct frames for `secs`.
fn sample(bridge: &mut Bridge, secs: f32) -> Result<Sample, String> {
    let mut out = Sample::default();
    let start = Instant::now();
    let mut last_frame = u64::MAX;
    while start.elapsed().as_secs_f32() < secs || out.frames == 0 {
        let stats = bridge.call("engine.stats", json!({"prefix": "render."}))?;
        let frame = stats.get("frame").and_then(Value::as_u64).unwrap_or(0);
        if frame == last_frame {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        last_frame = frame;
        out.frames += 1;
        out.frame_ms += stats.get("frame_ms").and_then(Value::as_f64).unwrap_or(0.0);
        if let Some(metrics) = stats.get("metrics").and_then(Value::as_object) {
            for (key, value) in metrics {
                // (The last line: the CPU's part of the rays' scene and how the probes' rays
                // went, for the `ray_scene_*` rows. Not passes: no table or sum takes them.)
                let wanted = key.starts_with("render.gpu")
                    || key.starts_with("render.extra_views_gpu")
                    || key == "render.extra_views_ms"
                    || RAY_ROWS.contains(&key.as_str());
                if let (true, Some(v)) = (wanted, value.as_f64()) {
                    *out.sums.entry(key.clone()).or_default() += v;
                }
            }
        }
        if start.elapsed().as_secs_f32() > secs.max(1.0) * 20.0 {
            break;
        }
    }
    Ok(out)
}

fn wait(bridge: &mut Bridge, secs: f32) -> Result<(), String> {
    // Pinging keeps the read alive and proves frames still run.
    let start = Instant::now();
    while start.elapsed().as_secs_f32() < secs {
        bridge.call("bridge.ping", json!({}))?;
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

struct Args {
    addr: String,
    stops: Vec<String>,
    only: Option<Vec<String>>,
    /// Switches left out (one that the game cannot take on this machine).
    skip: Vec<String>,
    dwell: f32,
    settle: f32,
    secs: f32,
    out: String,
    label: String,
    goto: bool,
    shots: bool,
    /// Switches to take a picture under ("all" for every one).
    variant_shots: Vec<String>,
    /// Take the pictures without the readings.
    shots_only: bool,
    /// Read everything against the frame as it was before the cuts became the default.
    old_way: bool,
    /// Bridge methods called once at each stop, their answers kept in the JSON.
    calls: Vec<String>,
    /// Prefixes of the game's numbers (`engine.stats`) kept in the JSON at each stop.
    stats: Vec<String>,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        addr: "127.0.0.1:7878".into(),
        stops: vec!["plaza".into()],
        only: None,
        skip: Vec::new(),
        dwell: 25.0,
        settle: 2.5,
        secs: 4.0,
        out: "gpu-sweep".into(),
        label: "default".into(),
        goto: true,
        shots: false,
        variant_shots: Vec::new(),
        shots_only: false,
        old_way: false,
        calls: Vec::new(),
        stats: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--addr" => a.addr = value("--addr")?,
            "--stops" => {
                a.stops = value("--stops")?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect();
            }
            "--variants" => {
                let list = value("--variants")?;
                a.only = Some(if list == "none" {
                    Vec::new()
                } else {
                    list.split(',').map(|s| s.trim().to_string()).collect()
                });
            }
            "--skip" => {
                a.skip = value("--skip")?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect();
            }
            "--dwell" => a.dwell = value("--dwell")?.parse().map_err(|_| "bad --dwell")?,
            "--settle" => a.settle = value("--settle")?.parse().map_err(|_| "bad --settle")?,
            "--secs" => a.secs = value("--secs")?.parse().map_err(|_| "bad --secs")?,
            "--out" => a.out = value("--out")?,
            "--label" => a.label = value("--label")?,
            "--no-goto" => a.goto = false,
            "--shots-only" => a.shots_only = true,
            "--old-way" => a.old_way = true,
            "--call" => {
                a.calls = value("--call")?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "--stats" => {
                a.stats = value("--stats")?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "--shots" => a.shots = true,
            "--variant-shots" => {
                a.shots = true;
                a.variant_shots = value("--variant-shots")?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect();
            }
            "--list" => {
                for v in variants() {
                    let times: Vec<String> =
                        v.times.iter().map(|(k, f)| format!("{k} x{f}")).collect();
                    let also: Vec<String> =
                        v.also.iter().map(|(r, p)| format!(" + {r} {p}")).collect();
                    println!(
                        "{:22} {} {} {}{}  ({})",
                        v.name,
                        v.resource,
                        v.patch,
                        times.join(" "),
                        also.concat(),
                        v.note
                    );
                }
                return Ok(None);
            }
            other => return Err(format!("unknown argument {other} (see the file's header)")),
        }
    }
    if !a.goto {
        a.stops = vec!["here".into()];
    }
    Ok(Some(a))
}

fn run(args: &Args) -> Result<(), String> {
    let mut bridge = Bridge::connect(&args.addr, Duration::from_secs(600))?;
    let all = variants();
    for n in args.only.iter().flatten().chain(&args.skip) {
        if !all.iter().any(|v| v.name == n) {
            return Err(format!("no variant {n} (--list)"));
        }
    }
    let chosen: Vec<&Variant> = all
        .iter()
        .filter(|v| {
            args.only
                .as_ref()
                .is_none_or(|names| names.iter().any(|n| n == v.name))
                && !args.skip.iter().any(|n| n == v.name)
        })
        .collect();
    let renderer = bridge
        .call("render.stats", json!({}))
        .unwrap_or(Value::Null);
    // `--old-way`: the baseline is the frame without any cut, so a cut's row reads what that
    // cut saves by itself and `cut_all` what the default frame saves. Put back at the end.
    let old = json!({"trial_cuts": 0, "trial_cuts_more": 0, "shadow_refresh_secs": 0.0, "pacing_fps": 0.0, "gi_hardware_rays": false});
    let default_way = if args.old_way {
        let now = bridge.call("resource.get", json!({"name": "RenderSettings"}))?;
        bridge.call(
            "resource.set",
            json!({"name": "RenderSettings", "value": old}),
        )?;
        Some(restore_of(&now, &old))
    } else {
        None
    };
    let mut results = Map::new();
    let mut md = String::new();
    md.push_str(&format!("# GPU sweep: {}\n\n", args.label));
    if args.old_way {
        md.push_str("Baseline: the old way (no cut on, the rays in software).\n\n");
    }
    let stats = &renderer["stats"];
    md.push_str(&format!(
        "GPU: {} | size: {}x{} | {} s a reading after {} s to settle\n\n",
        stats["adapter"].as_str().unwrap_or("?"),
        stats["width"],
        stats["height"],
        args.secs,
        args.settle
    ));
    for stop in &args.stops {
        if args.goto {
            if let Err(e) = bridge.call("showcase.goto", json!({"place": stop, "with_time": true}))
            {
                eprintln!("{stop}: skipped ({e})");
                continue;
            }
            eprintln!("{stop}: settling {} s", args.dwell);
            wait(&mut bridge, args.dwell)?;
        }
        if args.shots {
            shot(&mut bridge, &format!("{}-{stop}.png", args.out))?;
        }
        // Impostor bakes so far, before and after the first baseline: bakes during it are in
        // its "compute meshes: impostor bake" row and not in a settled frame.
        let bakes = |bridge: &mut Bridge| {
            let at = bridge.call("render.stats", json!({})).ok()?;
            at["renderer"]["features"]
                .as_array()?
                .iter()
                .find(|f| f["name"] == "meshes")?["state"]["impostor_bakes"]
                .as_u64()
        };
        let bakes_before = bakes(&mut bridge);
        let base = sample(&mut bridge, args.secs)?;
        let mut stop_json = Map::new();
        stop_json.insert("baseline".into(), base.to_json());
        stop_json.insert(
            "impostor_bakes_around_baseline".into(),
            json!([bakes_before, bakes(&mut bridge)]),
        );
        eprintln!(
            "{stop}: baseline gpu {:.2} ms, frame {:.2} ms",
            base.gpu_ms(),
            base.frame_ms()
        );
        let base_triangles = triangles(&mut bridge);
        let base_state = gi_and_memory(&mut bridge);
        if let Some(state) = &base_state {
            stop_json.insert("state".into(), state.clone());
        }
        // What the renderer reports of the stop (in view, meshes, ground, hair and fur), read
        // with the baseline: before any switch has changed what it draws or holds.
        let base_stats = bridge.call("render.stats", json!({})).ok();
        // The main thread's own time here, the soft bodies' numbers, and what was asked of
        // the game at each stop (`--call`).
        let main = main_thread(&mut bridge, 1.0);
        if let Some(main) = &main {
            stop_json.insert("main_thread".into(), main.clone());
        }
        if let Some(soft) = numbers(&mut bridge, "softbody.") {
            stop_json.insert("softbody".into(), soft);
        }
        // The mesh scene's slots, and how many of them the frame's sync wrote again or found
        // written without change: what "meshes: sync" on the main thread is made of.
        let mesh_scene = mean_numbers(&mut bridge, "render.meshes.", 0.5);
        if let Some((frames, means)) = &mesh_scene {
            stop_json.insert(
                "mesh_scene".into(),
                json!({"frames": frames, "means": means}),
            );
        }
        // Whatever else was asked for (`--stats`), as means over half a second.
        if let Some(asked) = asked_stats(&mut bridge, &args.stats, stop) {
            stop_json.insert("stats".into(), asked);
        }
        let mut answers = Map::new();
        for method in &args.calls {
            match bridge.call(method, json!({})) {
                Ok(answer) => {
                    answers.insert(method.clone(), answer);
                }
                Err(e) => eprintln!("{stop}: {method} not answered ({e})"),
            }
        }
        if !answers.is_empty() {
            stop_json.insert("calls".into(), Value::Object(answers));
        }
        // Every switch is read between two readings of the scene as it was (the one before it
        // and the one after it is switched back), so slow drift (clouds moving in, the sun,
        // people gathering) doesn't count as its cost.
        let mut before = base.clone();
        let mut baselines = vec![base.gpu_ms()];
        let mut rows: Vec<Row> = Vec::new();
        let timed: &[&Variant] = if args.shots_only { &[] } else { &chosen };
        for v in timed {
            let on = match switch_on(&mut bridge, v) {
                Ok(on) => on,
                Err(e) => {
                    eprintln!("{stop}: {} skipped ({e})", v.name);
                    continue;
                }
            };
            wait(&mut bridge, args.settle)?;
            let s = sample(&mut bridge, args.secs)?;
            let tris = triangles(&mut bridge);
            let state = gi_and_memory(&mut bridge);
            // (What was asked for with `--stats`, as it is while this row is on.)
            let row_stats = asked_stats(&mut bridge, &args.stats, &format!("{stop}, {}", v.name));
            // (`count_mesh_runs*` and `count_grass_runs*` rows.)
            let runs = ["mesh_runs", "grass_runs"]
                .into_iter()
                .find(|key| v.name.starts_with(&format!("count_{key}")))
                .and_then(|key| Some((key, runs_of(&mut bridge, key)?)));
            switch_back(&mut bridge, &on)?;
            wait(&mut bridge, args.settle)?;
            let after = sample(&mut bridge, args.secs)?;
            let around = Sample::mean_of(&before, &after);
            eprintln!(
                "{stop}: {:22} gpu {:.2} ms ({:+.2})",
                v.name,
                s.gpu_ms(),
                s.gpu_ms() - around.gpu_ms()
            );
            let mut entry = s.to_json();
            entry["paired_baseline_gpu_ms"] = json!(round3(around.gpu_ms()));
            entry["saves_ms"] = json!(round3(around.gpu_ms() - s.gpu_ms()));
            entry["triangles"] = json!(tris);
            if let Some(state) = state {
                entry["state"] = state;
            }
            if let Some(stats) = row_stats {
                entry["stats"] = stats;
            }
            if let Some((key, runs)) = runs {
                eprintln!("{stop}: {} {runs}", v.name);
                entry[key] = runs;
            }
            entry["patch"] = on.patch;
            if !on.also.is_empty() {
                entry["also"] = Value::Object(on.also);
            }
            stop_json.insert(v.name.into(), entry);
            baselines.push(after.gpu_ms());
            rows.push(Row {
                name: v.name,
                with: s,
                around,
                triangles: tris,
            });
            before = after;
        }
        let again = before;
        stop_json.insert("baseline_again".into(), again.to_json());
        // Pictures last, with the simulation paused, so a switch's picture differs from the
        // stop's own only by what the switch does (no one walks on between the two).
        let mut pictures: Vec<(String, Option<u64>)> = Vec::new();
        let pictured: Vec<&&Variant> = chosen
            .iter()
            .filter(|v| args.variant_shots.iter().any(|n| n == v.name || n == "all"))
            .collect();
        if !pictured.is_empty() {
            let paused = bridge.call("engine.pause", json!({})).is_ok();
            wait(&mut bridge, args.settle)?;
            shot(&mut bridge, &format!("{}-{stop}-paused.png", args.out))?;
            let still = triangles(&mut bridge);
            pictures.push(("paused".to_string(), still));
            for v in pictured {
                let Ok(on) = switch_on(&mut bridge, v) else {
                    continue;
                };
                wait(&mut bridge, args.settle)?;
                shot(&mut bridge, &format!("{}-{stop}-{}.png", args.out, v.name))?;
                pictures.push((v.name.to_string(), triangles(&mut bridge)));
                switch_back(&mut bridge, &on)?;
            }
            if paused {
                bridge.call("engine.resume", json!({}))?;
            }
            wait(&mut bridge, args.settle)?;
        }
        let (low, high) = baselines
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), b| (l.min(*b), h.max(*b)));

        md.push_str(&format!("## {stop}\n\n"));
        md.push_str(&format!(
            "Baseline: GPU {:.2} ms, frame {:.2} ms ({:.0} fps). Read again {} times between the switches: {:.2} to {:.2} ms.\n\n",
            base.gpu_ms(),
            base.frame_ms(),
            1000.0 / base.frame_ms().max(1e-3),
            baselines.len() - 1,
            low,
            high
        ));
        if let Some(main) = &main {
            let n = |key: &str| main.get(key).and_then(Value::as_f64).unwrap_or(-1.0);
            md.push_str(&format!(
                "Main thread: {:.1} ms of its own work a frame (fixed steps {:.1} ms), {:.1} ms waiting for the render thread; mean of {} frames.\n\n",
                n("main_ms"),
                n("fixed_ms"),
                n("wait_ms"),
                main["frames"]
            ));
        }
        if let Some(state) = &base_state {
            let n = |key: &str| state.get(key).and_then(Value::as_f64).unwrap_or(-1.0);
            md.push_str(&format!(
                "Probe GI's rays: {} (device ray queries {}, lost {} times); {} instances over {} triangles in structures, {} compacted, {} waiting to be freed. GPU memory: {:.0} MB in use, {:.0} MB reserved.\n\n",
                if n("render.gi.hardware_rays") == 1.0 {
                    "on the RT cores"
                } else {
                    "in software"
                },
                n("render.ray_queries"),
                n("render.gi.rt_losses"),
                n("render.gi.rt_instances"),
                n("render.gi.rt_triangles"),
                n("render.gi.rt_compacted"),
                n("render.gi.rt_retired"),
                n("render.gpu_memory_mb"),
                n("render.gpu_reserved_mb"),
            ));
        }
        if let Some(at) = &base_stats {
            let s = &at["stats"];
            md.push_str(&format!(
                "In view: {} triangles, {} draw calls, {} instances, {} point lights.\n\n",
                s["triangles"], s["draw_calls"], s["instances"], s["point_lights"]
            ));
            stop_json.insert("render_stats".into(), s.clone());
            // What the meshes report (impostors baked and in view, batches, the pool, virtual
            // geometry), to read "compute meshes" and the mesh rows against.
            if let Some(meshes) = at["renderer"]["features"]
                .as_array()
                .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
            {
                let m = &meshes["state"];
                md.push_str(&format!(
                    "Meshes: {} opaque batches, {} impostors baked, {} in view, {} fading ({} bakes so far); {} of {} skinned meshes posed this frame, {} vertices.

",
                    m["opaque_batches"],
                    m["impostors"],
                    m["impostors_in_view"],
                    m["impostors_fading"],
                    m["impostor_bakes"],
                    m["skinned"],
                    m["skins"],
                    m["skinned_vertices"]
                ));
                stop_json.insert("meshes".into(), m.clone());
            }
            if let Some((frames, means)) = &mesh_scene {
                let n = |key: &str| means.get(key).copied().unwrap_or(-1.0);
                md.push_str(&format!(
                    "Mesh scene: {:.0} slots; a frame's sync writes {:.0} of them again and finds {:.0} written without change (means of {frames} frames).\n\n",
                    n("render.meshes.slots"),
                    n("render.meshes.dirty_slots"),
                    n("render.meshes.unchanged_writes"),
                ));
            }
            // What the ground reports (patches resident and in the shadow maps' still images,
            // how often each image was drawn so far), to read the shadow rows against.
            if let Some(terrain) = at["renderer"]["features"]
                .as_array()
                .and_then(|f| f.iter().find(|f| f["name"] == "terrain"))
            {
                let t = &terrain["state"];
                md.push_str(&format!(
                    "Ground: {} patches resident, {} in the shadow maps' still images, each drawn {} times so far.\n\n",
                    t["patches_resident"], t["shadow_patches_still"], t["shadow_images_drawn"]
                ));
                stop_json.insert("terrain".into(), t.clone());
            }
            // What the hair and fur report (instances, strands drawn of those their grooms
            // hold, points), to read `before scene` against.
            if let Some(strands) = at["renderer"]["features"]
                .as_array()
                .and_then(|f| f.iter().find(|f| f["name"] == "strands"))
            {
                let t = &strands["state"];
                if t["instances"].as_u64().unwrap_or(0) > 0 {
                    md.push_str(&format!(
                        "Hair and fur: {} instances draw {} of their grooms' {} strands, {} points.\n\n",
                        t["instances"], t["strands"], t["strands_in_grooms"], t["points"]
                    ));
                }
                // The grooms the GPU holds: a groom is sent when an instance that wears it is
                // drawn, so the count sent stands still at a stop (it rose about once a second
                // until 17 Nov 2026, each time a frame of 0.3 s).
                if let Some(sent) = t["groom_uploads"].as_u64() {
                    md.push_str(&format!(
                        "Grooms on the GPU: {} in {:.0} MB of {:.0} MB; {} sent so far, {} dropped, arrays made {} times.\n\n",
                        t["grooms"],
                        t["groom_mb_used"].as_f64().unwrap_or(0.0),
                        t["groom_mb"].as_f64().unwrap_or(0.0),
                        sent,
                        t["groom_evictions"],
                        t["groom_repacks"]
                    ));
                }
                stop_json.insert("strands".into(), t.clone());
            }
        }
        md.push_str("| pass | ms |\n|---|---|\n");
        let mut passes: Vec<(&String, f64)> = base
            .sums
            .keys()
            .filter(|k| k.starts_with("render.gpu."))
            .map(|k| (k, base.mean(k)))
            .collect();
        passes.sort_by(|a, b| b.1.total_cmp(&a.1));
        // (Rows under half a percent of the frame are in the JSON.)
        let floor = (base.gpu_ms() * 0.005).max(0.02);
        for (key, ms) in passes.iter().filter(|(_, ms)| *ms >= floor) {
            md.push_str(&format!(
                "| {} | {:.2} |\n",
                key.trim_start_matches("render.gpu."),
                ms
            ));
        }
        if !rows.is_empty() {
            md.push_str(
                "\n| switched | GPU ms | saves | triangles | passes that moved most |\n|---|---|---|---|---|\n",
            );
            rows.sort_by(|a, b| b.saves().total_cmp(&a.saves()));
            for row in &rows {
                let mut moved: Vec<(String, f64)> = row
                    .around
                    .sums
                    .keys()
                    .chain(row.with.sums.keys())
                    .filter(|k| k.starts_with("render.gpu."))
                    .map(|k| (k.clone(), row.with.mean(k) - row.around.mean(k)))
                    .collect();
                moved.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
                moved.dedup_by(|a, b| a.0 == b.0);
                let top: Vec<String> = moved
                    .iter()
                    .filter(|(k, d)| d.abs() >= 0.05 && k != "render.gpu.scene")
                    .take(4)
                    .map(|(k, d)| format!("{} {:+.2}", k.trim_start_matches("render.gpu."), d))
                    .collect();
                // Triangles only where the switch changed them.
                let tris = match (row.triangles, base_triangles) {
                    (Some(t), Some(b)) if b > 0 && (t as f64 / b as f64 - 1.0).abs() > 0.03 => {
                        format!(
                            "{:.2} M ({:+.0}%)",
                            t as f64 / 1e6,
                            (t as f64 / b as f64 - 1.0) * 100.0
                        )
                    }
                    _ => String::new(),
                };
                md.push_str(&format!(
                    "| {} | {:.2} | {:.2} | {} | {} |\n",
                    row.name,
                    row.with.gpu_ms(),
                    row.saves(),
                    tris,
                    top.join(", ")
                ));
            }
        }
        if !pictures.is_empty() {
            md.push_str("\n| picture (simulation paused) | triangles |\n|---|---|\n");
            for (name, tris) in &pictures {
                md.push_str(&format!(
                    "| {}-{stop}-{name}.png | {} |\n",
                    args.out.rsplit(['/', '\\']).next().unwrap_or(&args.out),
                    tris.map_or("?".to_string(), |t| format!("{:.2} M", t as f64 / 1e6))
                ));
            }
            stop_json.insert(
                "pictures".into(),
                Value::Array(
                    pictures
                        .iter()
                        .map(|(name, tris)| json!({"name": name, "triangles": tris}))
                        .collect(),
                ),
            );
        }
        md.push('\n');
        results.insert(stop.clone(), Value::Object(stop_json));
        // After every stop, so a sweep that dies at a later one keeps what it measured.
        let json = json!({"label": args.label.clone(), "old_way": args.old_way, "renderer": renderer.clone(), "stops": results.clone()});
        std::fs::write(format!("{}.md", args.out), &md).map_err(|e| e.to_string())?;
        std::fs::write(
            format!("{}.json", args.out),
            serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        eprintln!(
            "wrote {0}.md and {0}.json ({1} stops)",
            args.out,
            results.len()
        );
    }
    if let Some(back) = default_way {
        bridge.call(
            "resource.set",
            json!({"name": "RenderSettings", "value": back}),
        )?;
    }
    Ok(())
}

fn main() {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => return,
        Err(e) => {
            eprintln!("gpu-sweep: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&args) {
        eprintln!("gpu-sweep: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restore_patch_holds_the_old_values_of_the_patched_keys() {
        let current = json!({"probe_gi": true, "render_scale": 1.0, "other": 3});
        let patch = json!({"probe_gi": false, "render_scale": 0.5});
        assert_eq!(
            restore_of(&current, &patch),
            json!({"probe_gi": true, "render_scale": 1.0})
        );
    }

    #[test]
    fn a_split_pass_counts_once_in_the_gpu_total() {
        let mut s = Sample {
            frames: 2,
            ..Default::default()
        };
        s.sums.insert("render.gpu.scene".into(), 8.0);
        s.sums.insert("render.gpu.scene: meshes".into(), 6.0);
        s.sums.insert("render.gpu.shadows".into(), 2.0);
        assert!((s.gpu_ms() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn a_times_variant_multiplies_the_value_as_it_stands() {
        let v = Variant {
            name: "t",
            resource: "R",
            patch: json!({"flag": false}),
            times: &[("size", 0.5), ("error", 2.0), ("missing", 2.0)],
            also: Vec::new(),
            note: "",
        };
        let current = json!({"size": 4096, "error": 0.75, "flag": true});
        assert_eq!(
            patch_against(&v.patch, v.times, &current),
            json!({"flag": false, "size": 2048, "error": 1.5})
        );
    }

    #[test]
    fn the_mean_of_two_readings_weighs_their_frames() {
        let mut a = Sample {
            frames: 1,
            ..Default::default()
        };
        a.sums.insert("render.gpu_ms".into(), 10.0);
        let mut b = Sample {
            frames: 3,
            ..Default::default()
        };
        b.sums.insert("render.gpu_ms".into(), 18.0);
        assert!((Sample::mean_of(&a, &b).gpu_ms() - 7.0).abs() < 1e-9);
    }

    /// Resources as the game keeps them, for `switch_on`: a merge patch sets the keys it names;
    /// every `set` is logged, and `refuse` names one that can't be set.
    #[derive(Default)]
    struct Game {
        resources: BTreeMap<String, Value>,
        sets: Vec<String>,
        refuse: Option<&'static str>,
    }

    impl Resources for Game {
        fn get(&mut self, name: &str) -> Result<Value, String> {
            self.resources
                .get(name)
                .cloned()
                .ok_or_else(|| format!("no resource {name}"))
        }

        fn set(&mut self, name: &str, value: &Value) -> Result<(), String> {
            if self.refuse == Some(name) {
                return Err(format!("{name} refused"));
            }
            let r = self.resources.get_mut(name).ok_or("no resource")?;
            for (key, v) in value.as_object().into_iter().flatten() {
                r[key] = v.clone();
            }
            self.sets.push(name.to_string());
            Ok(())
        }
    }

    #[test]
    fn a_row_of_two_resources_sets_both_and_puts_both_back_in_turn() {
        let v = Variant {
            name: "two",
            resource: "R",
            patch: json!({"trial_cuts_more": 272, "rays": true}),
            times: &[],
            also: vec![("W", json!({"mirror": false}))],
            note: "",
        };
        let before = [
            (
                "R",
                json!({"trial_cuts_more": 1, "rays": false, "other": 3}),
            ),
            ("W", json!({"mirror": true, "waves": 0.5})),
        ];
        let mut game = Game {
            resources: before
                .iter()
                .map(|(n, v)| (n.to_string(), v.clone()))
                .collect(),
            ..Default::default()
        };
        let on = switch_on(&mut game, &v).unwrap();
        // The cuts' bits join those already on; the second resource's patch is its own.
        assert_eq!(on.patch, json!({"trial_cuts_more": 273, "rays": true}));
        assert_eq!(
            on.also,
            json!({"W": {"mirror": false}})
                .as_object()
                .cloned()
                .unwrap()
        );
        assert_eq!(
            game.resources["R"],
            json!({"trial_cuts_more": 273, "rays": true, "other": 3})
        );
        assert_eq!(game.resources["W"], json!({"mirror": false, "waves": 0.5}));
        switch_back(&mut game, &on).unwrap();
        assert_eq!(game.sets, ["R", "W", "W", "R"]);
        for (name, value) in &before {
            assert_eq!(&game.resources[*name], value, "{name} put back");
        }

        // A second resource that can't be set skips the row, with the first put back.
        game.sets.clear();
        game.refuse = Some("W");
        let skipped = switch_on(&mut game, &v).err().unwrap();
        assert!(skipped.contains("W"), "{skipped}");
        assert_eq!(game.sets, ["R", "R"]);
        assert_eq!(game.resources["R"], before[0].1);
        // And one the game doesn't have.
        game.sets.clear();
        game.refuse = None;
        game.resources.remove("W");
        assert_eq!(switch_on(&mut game, &v).err().unwrap(), "no W");
        assert_eq!(game.sets, ["R", "R"]);
        assert_eq!(game.resources["R"], before[0].1);
    }

    #[test]
    fn variant_names_are_unique() {
        let mut names: Vec<_> = variants().iter().map(|v| v.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
    }
}
