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
//! or terrain setting), reads the rows again and switches it back. The difference between a
//! variant's total and the baseline's is what that thing costs at that stop, in milliseconds of
//! GPU time, with every other pass in the same state. The baseline is read again after every
//! variant, and a variant is compared with the two readings around it, so drift over the minutes
//! a sweep takes doesn't count. `--out NAME` writes `NAME.md` (tables to read) and `NAME.json`
//! (every number). `--shots` saves a picture of each stop, `--variant-shots a,b` (or `all`) one
//! under each of those variants too, as `NAME-<stop>[-<variant>].png`: those are taken after the
//! readings with the simulation paused, next to `NAME-<stop>-paused.png`, so two of them differ
//! only by what the switch does. `--shots-only` takes the pictures without the readings (a
//! software GPU's times say nothing).
//!
//! `--variants a,b` runs only those (names as in the table; `none` for the baseline alone, which
//! is what a launch with an environment switch such as `TERRAIN_SHADER_OFF=1` wants, with
//! `--label` to name it). `--list` prints the variants. Stops are `showcase.goto` places; with
//! `--no-goto` the camera stays where it is (any game, one "stop" named `here`).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

/// One thing to switch: a JSON merge patch for a registered resource.
struct Variant {
    name: &'static str,
    resource: &'static str,
    /// Values to set.
    patch: Value,
    /// Numbers to multiply instead: (key, factor) of the resource's value at the time.
    times: &'static [(&'static str, f64)],
    /// What it shows.
    note: &'static str,
}

impl Variant {
    /// The patch to send, given the resource as it stands: `patch` plus the `times` keys.
    fn patch_for(&self, current: &Value) -> Value {
        let mut out = self.patch.as_object().cloned().unwrap_or_default();
        for (key, factor) in self.times {
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
}

fn variants() -> Vec<Variant> {
    let r = "RenderSettings";
    let v = |name, resource, patch, note| Variant {
        name,
        resource,
        patch,
        times: &[],
        note,
    };
    // Multiplies numbers of the resource as it stands (thresholds a game or a preset set).
    let x = |name, resource, times, note| Variant {
        name,
        resource,
        patch: json!({}),
        times,
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
            "no_grass",
            r,
            json!({"grass_density": 0.0}),
            "grass and ground plants",
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
            "the ground textures' larger copies with one lookup, not the anti-tiling pair",
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

    /// The frame's GPU time: the profiler's own total where it reports one, else the passes
    /// summed (a split pass's parts, "scene: meshes", are inside "scene").
    fn gpu_ms(&self) -> f64 {
        if self.sums.contains_key("render.gpu_ms") {
            return self.mean("render.gpu_ms");
        }
        self.sums
            .keys()
            .filter(|k| k.starts_with("render.gpu.") && !k.contains(": "))
            .map(|k| self.mean(k))
            .sum()
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

/// Triangles drawn in the last frame.
fn triangles(bridge: &mut Bridge) -> Option<u64> {
    bridge.call("render.stats", json!({})).ok()?["stats"]["triangles"].as_u64()
}

/// The meshes' exact shader runs of the last frame read back (`shade_skip::COUNT_MESH_RUNS`).
fn mesh_runs(bridge: &mut Bridge) -> Option<Value> {
    let at = bridge.call("render.stats", json!({})).ok()?;
    at["renderer"]["features"]
        .as_array()?
        .iter()
        .map(|f| &f["state"]["mesh_runs"])
        .find(|runs| runs.is_object())
        .cloned()
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

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
                let wanted = key.starts_with("render.gpu")
                    || key.starts_with("render.extra_views_gpu")
                    || key == "render.extra_views_ms";
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
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        addr: "127.0.0.1:7878".into(),
        stops: vec!["plaza".into()],
        only: None,
        dwell: 25.0,
        settle: 2.5,
        secs: 4.0,
        out: "gpu-sweep".into(),
        label: "default".into(),
        goto: true,
        shots: false,
        variant_shots: Vec::new(),
        shots_only: false,
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
            "--dwell" => a.dwell = value("--dwell")?.parse().map_err(|_| "bad --dwell")?,
            "--settle" => a.settle = value("--settle")?.parse().map_err(|_| "bad --settle")?,
            "--secs" => a.secs = value("--secs")?.parse().map_err(|_| "bad --secs")?,
            "--out" => a.out = value("--out")?,
            "--label" => a.label = value("--label")?,
            "--no-goto" => a.goto = false,
            "--shots-only" => a.shots_only = true,
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
                    println!(
                        "{:22} {} {} {}  ({})",
                        v.name,
                        v.resource,
                        v.patch,
                        times.join(" "),
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
    let chosen: Vec<&Variant> = match &args.only {
        None => all.iter().collect(),
        Some(names) => {
            for n in names {
                if !all.iter().any(|v| v.name == n) {
                    return Err(format!("no variant {n} (--list)"));
                }
            }
            all.iter()
                .filter(|v| names.iter().any(|n| n == v.name))
                .collect()
        }
    };
    let renderer = bridge
        .call("render.stats", json!({}))
        .unwrap_or(Value::Null);
    let mut results = Map::new();
    let mut md = String::new();
    md.push_str(&format!("# GPU sweep: {}\n\n", args.label));
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
        let base = sample(&mut bridge, args.secs)?;
        let mut stop_json = Map::new();
        stop_json.insert("baseline".into(), base.to_json());
        eprintln!(
            "{stop}: baseline gpu {:.2} ms, frame {:.2} ms",
            base.gpu_ms(),
            base.frame_ms()
        );
        let base_triangles = triangles(&mut bridge);
        // Every switch is read between two readings of the scene as it was (the one before it
        // and the one after it is switched back), so slow drift (clouds moving in, the sun,
        // people gathering) doesn't count as its cost.
        let mut before = base.clone();
        let mut baselines = vec![base.gpu_ms()];
        let mut rows: Vec<Row> = Vec::new();
        let timed: &[&Variant] = if args.shots_only { &[] } else { &chosen };
        for v in timed {
            let current = bridge.call("resource.get", json!({"name": v.resource}));
            let Ok(current) = current else {
                eprintln!("{stop}: {} skipped (no {})", v.name, v.resource);
                continue;
            };
            let patch = v.patch_for(&current);
            let back = restore_of(&current, &patch);
            if let Err(e) = bridge.call("resource.set", json!({"name": v.resource, "value": patch}))
            {
                eprintln!("{stop}: {} skipped ({e})", v.name);
                continue;
            }
            wait(&mut bridge, args.settle)?;
            let s = sample(&mut bridge, args.secs)?;
            let tris = triangles(&mut bridge);
            let runs = if v.name.starts_with("count_mesh_runs") {
                mesh_runs(&mut bridge)
            } else {
                None
            };
            bridge.call("resource.set", json!({"name": v.resource, "value": back}))?;
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
            if let Some(runs) = runs {
                eprintln!("{stop}: {} {runs}", v.name);
                entry["mesh_runs"] = runs;
            }
            entry["patch"] = patch;
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
                let Ok(current) = bridge.call("resource.get", json!({"name": v.resource})) else {
                    continue;
                };
                let patch = v.patch_for(&current);
                let back = restore_of(&current, &patch);
                if bridge
                    .call("resource.set", json!({"name": v.resource, "value": patch}))
                    .is_err()
                {
                    continue;
                }
                wait(&mut bridge, args.settle)?;
                shot(&mut bridge, &format!("{}-{stop}-{}.png", args.out, v.name))?;
                pictures.push((v.name.to_string(), triangles(&mut bridge)));
                bridge.call("resource.set", json!({"name": v.resource, "value": back}))?;
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
        if let Ok(at) = bridge.call("render.stats", json!({})) {
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
                    "Meshes: {} opaque batches, {} impostors baked, {} in view, {} fading.

",
                    m["opaque_batches"],
                    m["impostors"],
                    m["impostors_in_view"],
                    m["impostors_fading"]
                ));
                stop_json.insert("meshes".into(), m.clone());
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
    }
    let json = json!({"label": args.label, "renderer": renderer, "stops": results});
    std::fs::write(format!("{}.md", args.out), &md).map_err(|e| e.to_string())?;
    std::fs::write(
        format!("{}.json", args.out),
        serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    eprintln!("wrote {0}.md and {0}.json", args.out);
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
            note: "",
        };
        let current = json!({"size": 4096, "error": 0.75, "flag": true});
        assert_eq!(
            v.patch_for(&current),
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

    #[test]
    fn variant_names_are_unique() {
        let mut names: Vec<_> = variants().iter().map(|v| v.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
    }
}
