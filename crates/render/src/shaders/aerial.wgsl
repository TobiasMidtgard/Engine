// The aerial perspective table (aerial.rs): one thread per screen column marches the column's
// ray from the camera to the table's reach and writes, at the far edge of every depth slice,
// the height fog beyond the volumetric fog's range and the atmosphere between the camera and
// there, as `surface_fog` took them per pixel. After Hillaire 2020 ("A Scalable and Production
// Ready Sky and Atmosphere Rendering Technique", section 5.4). Requires lib_types + lib_frame +
// lib_atmosphere; group 0 is the prepass frame group (binding 24 is a stand-in while the table
// is written).
//
// Parts, side by side along x (C columns each): height fog light (pre-exposed) and
// transmittance; atmosphere light (pre-exposed, times the sun's color) and red transmittance;
// atmosphere green and blue transmittance.

@group(1) @binding(0) var aerial_out: texture_storage_3d<rgba16float, write>;

// Haze shadow samples per slice: the slices together sample the far shadow map and the cloud
// shadows all along the ray (a pixel took 8 for its whole ray).
const SLICE_SHADOW_SAMPLES: i32 = 2;
// Atmosphere steps from the camera to each slice's far edge (as a pixel marched it).
const AIR_STEPS: i32 = 8;

@compute @workgroup_size(8, 8, 1)
fn cs_aerial(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(aerial_out);
    let columns = size.x / 3u;
    if (id.x >= columns || id.y >= size.y) { return; }
    // The column's ray through the unjittered view, and meters along it per meter of depth.
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(f32(columns), f32(size.y));
    let ray = (frame.inv_view * vec4<f32>(view_ray_at_uv(uv), 0.0)).xyz;
    let per_depth = length(ray);
    let dir = ray / per_depth;
    // The height fog starts where the volumetric fog's range (a view depth) ends.
    var fog_start = 0.0;
    if (frame.volumetrics.x > 0.0) { fog_start = frame.volumetrics.x * per_depth; }
    var fog = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var prev = 0.0;
    for (var k = 0u; k < size.z; k++) {
        let t = aerial_slice_distance(f32(k + 1u));
        let a = max(prev, fog_start);
        if (t > a) {
            let tr = exp(-height_fog_optical_depth(dir, a, t));
            if (tr < 0.9999) {
                // As `surface_fog` lights the height fog: haze in caves and rooms sees neither
                // sky nor sun, haze in a mountain's or a cloud's shadow doesn't glow.
                let lit = fog_sky_visibility(dir * t);
                let sun = height_fog_sun_visibility_n(dir, a, t, tr, 0.5, SLICE_SHADOW_SAMPLES);
                let at = height_fog_scatter_point(dir, a, t, tr);
                let in_fog = height_fog_sunlight_along(dir, a, t, tr);
                let l = pre_expose(height_fog_lit(dir, at, in_fog, lit, sun)) * (1.0 - tr);
                fog = vec4<f32>(fog.rgb + l * fog.a, fog.a * tr);
            }
        }
        let air = atmosphere(vec3<f32>(0.0), dir, t, AIR_STEPS);
        let light = pre_expose(air.radiance * frame.sun_color.rgb);
        textureStore(aerial_out, vec3<u32>(id.x, id.y, k), fog);
        textureStore(aerial_out, vec3<u32>(id.x + columns, id.y, k), vec4<f32>(light, air.transmittance.r));
        textureStore(aerial_out, vec3<u32>(id.x + 2u * columns, id.y, k), vec4<f32>(air.transmittance.gb, 0.0, 0.0));
        prev = t;
    }
}
