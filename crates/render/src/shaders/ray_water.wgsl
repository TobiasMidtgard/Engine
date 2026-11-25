// The water's own reflection ray (`trial_cut_more::RAY_WATER`, step 12 part two): after lib_ray at
// group 3 and ray_hit_light.wgsl (`ray_scene::water_reflection_lib`), before a fragment shader
// composed with lib_frame whose pipeline has `GpuShared::ray_group_layout` at 3 (the water
// bodies' ray pipeline). Its stub, ray_water_stub.wgsl (`ray_scene::WATER_REFLECTION_STUB`),
// finds nothing: change their `ray_reflection` together.

// What the ray sees: every class a view draws but the water's stand-ins (flat and one-sided at
// their still level: a ray from a trough would meet one's back, and the water draws its own).
// It asks its candidates by `WATER_RAY_RULES`, which `ray_scene::water_reflection_lib` puts in
// front: a reflection's own (`REFLECT_RULES`), or on a software device 0 (every triangle whole).
const WATER_RAY_SEES: u32 = RAY_DRAWN & ~RAY_WATER;
// The albedo of the walls the indoor volumes stand for (probe_gi.wgsl `VOLUME_WALL_ALBEDO`).
const WATER_RAY_WALL_ALBEDO: f32 = 0.3;

// The reflection along unit `r` of the water at camera-relative `rel`, of perceptual roughness
// `rough`, from the frame's ray scene: pre-exposed rgb, and in a 1 where it met something within
// a world reflection's reach (`400 exp(-5 rough)` m, at most 384) or a wall an indoor volume
// stands for. 0 where it met nothing, started inside something, points below the horizon (it
// would meet the water itself), or the water is rougher than `ssr_max_roughness`: there the
// screen's march and the sky stand. Its hit is lit as every pixel's reflection lights its hits
// (ray_hit_light.wgsl `reflected_hit_light`: the sun, the lamps on water this smooth, the probes
// or the sky's guess, its glow and its own shine, and the height fog's part), and what it meets
// is fogged from the camera to it, as every pixel's reflection fogs its hits and as the screen's
// march holds what it finds (`reflected_camera_fog`; the water's own fog then covers the camera's
// leg to the water over both, as ray_reflect.wgsl `reflect_full_pixel` has it and flips with
// it).
fn ray_reflection(rel: vec3<f32>, r: vec3<f32>, rough: f32) -> vec4<f32> {
    if (rough > frame.reflections.z || r.y <= 0.0) { return vec4<f32>(0.0); }
    // Off the surface by as much as the mirror's picture lifts its rays (ray_mirror.wgsl).
    let origin = rel + vec3<f32>(0.0, max(0.005, 1e-4 * length(rel)), 0.0);
    let t_max = min(384.0, 400.0 * exp(-5.0 * rough));
    let hit = ray_nearest(origin, r, t_max, WATER_RAY_SEES, WATER_RAY_RULES);
    let wall = sky_volume_wall(origin, r, hit.t);
    if (wall < SKY_VOLUME_NO_WALL) {
        // A wall of an indoor volume (a cave's rock): lit by the probes around it.
        let g = probe_gi(origin + r * wall, -r, -r);
        let lit = WATER_RAY_WALL_ALBEDO * INV_PI * g.rgb * g.a;
        let fogged = reflected_camera_fog(lit, origin + r * wall, frame_noise(pixel_of(rel)));
        return vec4<f32>(pre_expose(fogged), 1.0);
    }
    if (hit.row == RAY_NONE) { return vec4<f32>(0.0); }
    // The ray's cone: a pixel across where it starts, widening by a pixel's angle and the
    // lobe's (as wide all along when orthographic), which picks the level of the hit's tile.
    let alpha = rough * rough;
    let pixel_m = 2.0 / (abs(frame.proj[1][1]) * frame.viewport.y);
    let depth = max(view_depth(rel), 0.0);
    let cone = select(vec2<f32>(pixel_m * depth, pixel_m + alpha), vec2<f32>(pixel_m, alpha), frame.exposure.w > 0.5);
    let s = ray_surface(hit, origin, r, cone);
    // The back of a single-sided surface: the ray started inside something.
    if (s.inside) { return vec4<f32>(0.0); }
    let light = reflected_hit_light(origin, r, hit.t, s, rough);
    let lit = reflected_camera_fog(light, origin + r * hit.t, frame_noise(pixel_of(rel)));
    return vec4<f32>(pre_expose(min(lit, vec3<f32>(60000.0))), 1.0);
}
