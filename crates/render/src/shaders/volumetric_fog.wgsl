// Volumetric fog: a froxel grid (frustum-shaped cells fitted to the view) holding the height fog
// and the fog volumes, lit by the sun with its shadows, the sky and point lights, and integrated
// front to back. After Wronski 2014 ("Volumetric Fog: Unified compute shader based solution to
// atmospheric scattering") and Hillaire 2015 ("Physically Based and Unified Volumetric Rendering
// in Frostbite"). Requires lib_types + lib_frame + lib_atmosphere; group 0 is the prepass frame
// group (binding 19 is a stand-in while the volume is written).
//
// cs_scatter, one thread per froxel: evaluates the medium (the height fog exactly along the
// column's center ray, fog volumes at a point jittered inside the froxel, a different point every
// frame), lights it at that point (from above the ground at its pixel when just below it, see
// `lit_point`) and blends it with last frame's value at the same place in the world
// (temporal reprojection, 90% history), so each froxel converges to its average over its whole
// cell and nothing bands or flickers. Runs after the depth prepass. Output: light scattered
// toward the camera per meter (cd/m^2 per m, pre-exposed) and extinction (1/m).
//
// cs_integrate, one thread per column: marches the slices front to back with the energy
// conserving integral of each slice. Output: in-scattered light (pre-exposed) and transmittance
// from the camera to the far edge of every slice, which `apply_fog` reads.

struct Froxels {
    grid: vec4<u32>,   // columns, rows, depth slices, fog volume count
    jitter: vec4<f32>, // sample position inside the froxel this frame (0..1) xyz, w: history weight (0 = none)
    params: vec4<f32>, // history scale (this frame's pre-exposure / the history's), unused, unused, unused
};

// A `FogVolume` (GpuFogVolume in volumetrics.rs).
struct FogVolume {
    to_local: array<vec4<f32>, 3>, // rows of camera-relative -> unit shape coordinates (|q| < 1 inside)
    shape: vec4<f32>,              // half extents xyz (m), 0 box / 1 ellipsoid
    medium: vec4<f32>,             // density (1/m), edge falloff (m), anisotropy, noise strength
    albedo: vec4<f32>,             // albedo rgb, noise scale (m)
    emission: vec4<f32>,           // emitted radiance per meter rgb (cd/m^2 per m), unused
    bounds: vec4<f32>,             // bounding sphere: camera-relative center xyz, radius (m)
};

@group(1) @binding(0) var<uniform> froxels: Froxels;
// cs_scatter
@group(1) @binding(1) var<storage, read> fog_volumes: array<FogVolume>;
@group(1) @binding(2) var scatter_history: texture_3d<f32>;
@group(1) @binding(3) var scatter_out: texture_storage_3d<rgba16float, write>;
// cs_integrate
@group(1) @binding(4) var scatter_in: texture_3d<f32>;
@group(1) @binding(5) var integrated_out: texture_storage_3d<rgba16float, write>;
// cs_scatter: this frame's depth prepass (depth, reverse-Z, 0 = nothing; octahedral normals).
@group(1) @binding(6) var scene_depth: texture_depth_2d;
@group(1) @binding(7) var scene_normal: texture_2d<f32>;

const BLOCK: u32 = 4u;            // cs_scatter workgroups are blocks of 4x4x4 froxels
const MAX_FOG_LIGHTS: u32 = 256u; // point lights considered (the nearest: the buffer is sorted)
const MAX_FOG_VOLUMES: u32 = 64u;

// Shared by the froxels of a block.
var<workgroup> block_corners: array<vec3<f32>, 8>;
var<workgroup> block_sky: array<f32, 8>;          // sky occlusion map visibility at the corners
var<workgroup> block_sky_light: vec3<f32>;        // `fog_sky_light`
var<workgroup> light_mask: array<atomic<u32>, 8>;  // point lights reaching the block
var<workgroup> volume_mask: array<atomic<u32>, 2>; // fog volumes overlapping the block

// View depth of slice boundary `s` (0 = the camera, slices = the range): the inverse of the
// mapping in `volumetric_fog_at`.
fn slice_depth(s: f32) -> f32 {
    let v = frame.volumetrics;
    return (exp2(s / (v.w * v.z)) - 1.0) / v.y;
}

// Camera-relative position at froxel coordinates `c` (column, row, slice; continuous). The grid
// follows the unjittered view.
fn froxel_position(c: vec3<f32>) -> vec3<f32> {
    let uv = c.xy / vec2<f32>(froxels.grid.xy);
    let d = slice_depth(c.z);
    let view = vec3<f32>(
        (uv.x * 2.0 - 1.0) * d / frame.proj[0][0],
        (1.0 - uv.y * 2.0) * d / frame.proj[1][1],
        -d,
    );
    return (frame.inv_view * vec4<f32>(view, 0.0)).xyz;
}

// Where to light the fog at `rel` from (sun shadows, the sunlight's way through the air, sky
// cover), froxel coordinates `c`. A point just below the ground seen at its pixel is fog nobody
// sees through that pixel, but the neighboring pixels whose ground lies a little farther away
// read the froxel at that depth through the blended lookup: for them it continues the air lying
// on the ground. So it is lit from just above the ground (lifted out along its normal), as if
// the ground weren't there; otherwise the ground's own shadow and cover darken the fog on it in
// bands, one per row of froxels. Only surfaces facing up are lifted from (ground, floors, the
// tops of things), which never shade the air on them: a trunk or a wall lit from behind does
// shade the air in front of it, and air lifted sideways out of that shadow would glow over its
// edge. Points farther below (the air behind a hill) keep their own light.
fn lit_point(c: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth));
    let px = clamp(vec2<i32>(c.xy / vec2<f32>(froxels.grid.xy) * vec2<f32>(size)), vec2<i32>(0), size - 1);
    let z = textureLoad(scene_depth, px, 0);
    if (z <= 0.0) { return rel; }
    let nw = textureLoad(scene_normal, px, 0);
    if (nw.w < 0.5) { return rel; }
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    let surface = scene_linear_depth(uv, z);
    let depth = slice_depth(c.z);
    if (depth <= surface) { return rel; }
    let dist = length(rel);
    let v = rel / max(dist, 1e-6);
    var n = oct_decode(nw.xy);
    n = select(n, -n, dot(n, v) > 0.0); // facing the camera
    var up = vec3<f32>(0.0, 1.0, 0.0);
    if (frame.planet.w > 0.0) { up = normalize(rel - frame.planet.xyz); }
    let facing_up = smoothstep(0.3, 0.6, dot(n, up));
    if (facing_up <= 0.0) { return rel; }
    // How far below the surface, along its normal; neighbors' surfaces lie within about a row of
    // froxels (in angle) of this one.
    let below = (depth - surface) * (dist / depth) * abs(dot(n, v));
    let row_angle = 2.0 / (frame.proj[1][1] * f32(froxels.grid.y));
    if (below > 3.0 * surface * row_angle + 0.5) { return rel; }
    // Clear of the surface's own shadow-map texels.
    return rel + n * ((below + 0.25 + 0.002 * surface) * facing_up);
}

// Sun visibility for a point in the air from the shadow cascades: one filtered tap. The point
// moves inside its froxel every frame, which softens the edges of the shafts over time.
fn volume_shadow(rel: vec3<f32>) -> f32 {
    if (frame.sun_color.w < 0.5) { return 1.0; }
    let depth = view_depth(rel);
    if (depth > frame.shadow_splits[3]) { return 1.0; }
    var cascade = 0u;
    for (var i = 0u; i < 3u; i++) {
        if (depth > frame.shadow_splits[i]) { cascade = i + 1u; }
    }
    let lp = frame.shadow_cascades[cascade] * vec4<f32>(rel, 1.0);
    let uv = lp.xy * vec2<f32>(0.5, -0.5) + 0.5;
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0)) || lp.z > 1.0) { return 1.0; }
    let lit = textureSampleCompareLevel(shadow_map, shadow_sampler, uv, cascade, lp.z - frame.shadow_params.x);
    let fade = saturate((frame.shadow_splits[3] - depth) / (frame.shadow_splits[3] * 0.1));
    return mix(1.0, lit, fade);
}

// How much of fog volume `v`'s density is at camera-relative `rel`: 0 outside, fading in over
// the edge falloff, varied by the drifting noise (0..2 at full noise).
fn fog_volume_amount(v: FogVolume, rel: vec3<f32>) -> f32 {
    let p = vec4<f32>(rel, 1.0);
    let q = vec3<f32>(dot(v.to_local[0], p), dot(v.to_local[1], p), dot(v.to_local[2], p));
    var inside: f32; // meters to the surface
    if (v.shape.w < 0.5) {
        let a = abs(q);
        if (any(a >= vec3<f32>(1.0))) { return 0.0; }
        let to_face = (1.0 - a) * v.shape.xyz;
        inside = min(to_face.x, min(to_face.y, to_face.z));
    } else {
        let r = length(q);
        if (r >= 1.0) { return 0.0; }
        inside = (1.0 - r) * min(v.shape.x, min(v.shape.y, v.shape.z));
    }
    var amount = smoothstep(0.0, 1.0, inside / max(v.medium.y, 1e-3));
    if (v.medium.w > 0.0) {
        // Wisps `noise scale` meters across, carried by the wind.
        let n = fbm3((tiling_pos(rel) - frame.wind.xyz * frame.time.x) / v.albedo.w, 3);
        amount *= mix(1.0, 2.0 * saturate((n - 0.4375) * 4.0 + 0.5), v.medium.w);
    }
    return amount;
}

@compute @workgroup_size(4, 4, 4)
fn cs_scatter(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
    @builtin(workgroup_id) block: vec3<u32>,
) {
    let grid = froxels.grid.xyz;
    // The block's corners, and the sky visibility there. The occlusion map lookup is the most
    // expensive part of lighting a froxel and changes slowly, so it is interpolated.
    if (local < 8u) {
        let corner = vec3<u32>(local & 1u, (local >> 1u) & 1u, local >> 2u);
        let c = vec3<f32>((block + corner) * BLOCK);
        let p = froxel_position(c);
        block_corners[local] = p;
        block_sky[local] = sky_map_visibility(lit_point(c, p), vec3<f32>(0.0, 1.0, 0.0));
        atomicStore(&light_mask[local], 0u);
        if (local < 2u) { atomicStore(&volume_mask[local], 0u); }
    } else if (local == 8u) {
        block_sky_light = fog_sky_light();
    }
    workgroupBarrier();

    // Point lights and fog volumes that reach the block (its bounding sphere).
    var center = vec3<f32>(0.0);
    for (var i = 0u; i < 8u; i++) { center += block_corners[i] * 0.125; }
    var radius = 0.0;
    for (var i = 0u; i < 8u; i++) { radius = max(radius, distance(block_corners[i], center)); }
    let lights = min(frame.counts.x, MAX_FOG_LIGHTS);
    for (var i = local; i < lights; i += BLOCK * BLOCK * BLOCK) {
        let l = point_lights[i].position_range;
        if (distance(l.xyz, center) < l.w + radius) {
            atomicOr(&light_mask[i / 32u], 1u << (i % 32u));
        }
    }
    if (local < min(froxels.grid.w, MAX_FOG_VOLUMES)) {
        let b = fog_volumes[local].bounds;
        if (distance(b.xyz, center) < b.w + radius) {
            atomicOr(&volume_mask[local / 32u], 1u << (local % 32u));
        }
    }
    workgroupBarrier();
    if (any(id >= grid)) { return; }

    // This frame's sample point in the froxel. The depth offset also varies between columns, so
    // sharp edges (a fog volume's face) dither across slices instead of banding.
    let jitter = vec3<f32>(
        froxels.jitter.xy,
        fract(froxels.jitter.z + interleaved_gradient_noise(vec2<f32>(id.xy))),
    );
    let rel = froxel_position(vec3<f32>(id) + jitter);
    let lit_at = lit_point(vec3<f32>(id) + jitter, rel);

    // The medium: height fog plus the fog volumes here. Fog volumes fade out over the last 15%
    // of the range (nothing draws them beyond it), so they never end in a hard edge; the height
    // fog continues analytically. The height fog is its exact mean over the froxel's stretch of
    // the column's center ray: lookups correct it per pixel against exactly that
    // (`volumetric_fog_at`), and a jittered sample's noise would show through.
    let range = frame.volumetrics.x;
    let volume_fade = 1.0 - smoothstep(0.85 * range, range, slice_depth(f32(id.z) + jitter.z));
    let column = vec2<f32>(id.xy) + 0.5;
    let enter = froxel_position(vec3<f32>(column, f32(id.z)));
    let leave = froxel_position(vec3<f32>(column, f32(id.z) + 1.0));
    let span = vec2<f32>(length(enter), length(leave));
    let optical_depth = height_fog_optical_depth(leave / max(span.y, 1e-6), span.x, span.y);
    var extinction = optical_depth / max(span.y - span.x, 1e-6);
    var scattering = extinction * frame.fog_color.rgb;
    var g_sum = luminance(scattering) * frame.fog_color.w;
    var light = vec3<f32>(0.0); // scattered toward the camera per meter (cd/m^2 per m)
    for (var word = 0u; word < 2u; word++) {
        var bits = atomicLoad(&volume_mask[word]);
        if (volume_fade <= 0.0) { bits = 0u; }
        while (bits != 0u) {
            let v = fog_volumes[word * 32u + firstTrailingBit(bits)];
            bits &= bits - 1u;
            let amount = fog_volume_amount(v, rel) * volume_fade;
            if (amount <= 0.0) { continue; }
            let density = v.medium.x * amount;
            let s = density * v.albedo.rgb;
            extinction += density;
            scattering += s;
            g_sum += luminance(s) * v.medium.z;
            light += v.emission.rgb * amount;
        }
    }

    let s_lum = luminance(scattering);
    if (s_lum > 0.0) {
        // One phase function for the mix of media: the scattering-weighted anisotropy.
        let g = clamp(g_sum / s_lum, -0.95, 0.95);
        let view_dir = rel / max(length(rel), 1e-6);
        var incoming = vec3<f32>(0.0); // radiance arriving, weighted by the phase function
        // Sun, through the air and the shadow cascades.
        if (any(frame.sun_color.rgb > vec3<f32>(0.0))) {
            let shadow = volume_shadow(lit_at);
            if (shadow > 0.0) {
                let phase = henyey_greenstein(dot(view_dir, frame.sun_dir.xyz), g);
                incoming += frame.sun_color.rgb * sun_transmittance(lit_at) * (shadow * phase);
            }
        }
        // Sky, scattered about evenly; dark in caves and rooms. The occlusion map's visibility is
        // interpolated between the block's corners, the indoor volumes are exact.
        let f = (vec3<f32>(id - block * BLOCK) + jitter) / f32(BLOCK);
        let y0 = mix(mix(block_sky[0], block_sky[1], f.x), mix(block_sky[2], block_sky[3], f.x), f.y);
        let y1 = mix(mix(block_sky[4], block_sky[5], f.x), mix(block_sky[6], block_sky[7], f.x), f.y);
        let sky_vis = mix(y0, y1, f.z) * sky_volume_factor(lit_at);
        incoming += block_sky_light * sky_vis + frame.ambient.rgb;
        // Point lights, with the falloff surfaces get (`shade_surface`); no shadows.
        for (var word = 0u; word < 8u; word++) {
            var bits = atomicLoad(&light_mask[word]);
            while (bits != 0u) {
                let pl = point_lights[word * 32u + firstTrailingBit(bits)];
                bits &= bits - 1u;
                let to_light = pl.position_range.xyz - rel;
                let dist2 = dot(to_light, to_light);
                let range2 = sq(pl.position_range.w);
                if (dist2 >= range2) { continue; }
                let falloff = sq(saturate(1.0 - sq(dist2 / range2))) / max(dist2, sq(pl.color_radius.w));
                let cos_t = dot(view_dir, to_light) * inverseSqrt(max(dist2, 1e-12));
                incoming += pl.color_radius.rgb * (falloff * henyey_greenstein(cos_t, g));
            }
        }
        light += scattering * incoming;
    }

    var value = vec4<f32>(min(pre_expose(light), vec3<f32>(60000.0)), extinction);
    // One bad froxel must never spread through the history.
    if (any(value != value)) { value = vec4<f32>(0.0); }

    // Blend with last frame's froxels at the same place in the world.
    if (froxels.jitter.w > 0.0) {
        let prev_rel = froxel_position(vec3<f32>(id) + 0.5) + frame.camera_delta.xyz;
        let clip = frame.prev_view_proj * vec4<f32>(prev_rel, 1.0);
        if (clip.w > 1e-4) {
            let uv = clip.xy / clip.w * vec2<f32>(0.5, -0.5) + 0.5;
            let v = frame.volumetrics;
            let w = log2(1.0 + clip.w * v.y) * v.z;
            if (all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0)) && w <= 1.0) {
                let h = textureSampleLevel(scatter_history, linear_clamp, vec3<f32>(uv, w), 0.0);
                value = mix(value, vec4<f32>(h.rgb * froxels.params.x, h.a), froxels.jitter.w);
            }
        }
    }
    textureStore(scatter_out, vec3<i32>(id), value);
}

@compute @workgroup_size(8, 8, 1)
fn cs_integrate(@builtin(global_invocation_id) id: vec3<u32>) {
    let grid = froxels.grid.xyz;
    if (id.x >= grid.x || id.y >= grid.y) { return; }
    // Meters along the view ray per meter of view depth, through the column's center.
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(grid.xy);
    let stretch = length(vec3<f32>(
        (uv.x * 2.0 - 1.0) / frame.proj[0][0],
        (1.0 - uv.y * 2.0) / frame.proj[1][1],
        1.0,
    ));
    var light = vec3<f32>(0.0);
    var transmittance = 1.0;
    var near = 0.0;
    for (var z = 0u; z < grid.z; z++) {
        let far = slice_depth(f32(z + 1u));
        let len = (far - near) * stretch;
        near = far;
        let cell = vec3<i32>(vec3<u32>(id.xy, z));
        let s = textureLoad(scatter_in, cell, 0);
        // Energy-conserving integral over the slice (Hillaire 2015): light scattered anywhere in
        // the slice is dimmed by the rest of the slice on its way out,
        // integral of S e^(-sigma t) dt over 0..len = S (1 - e^(-sigma len)) / sigma.
        let x = max(s.a, 0.0) * len;
        let tr = exp(-x);
        let f = select((1.0 - tr) / max(x, 1e-12), 1.0 - 0.5 * x, x < 1e-3);
        light += s.rgb * (transmittance * len * f);
        transmittance *= tr;
        textureStore(integrated_out, cell, vec4<f32>(light, transmittance));
    }
}
