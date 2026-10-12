// Sky visibility at the camera from the occlusion map, once per frame (`fog_sky_visibility`
// reads it from frame binding 9). Composed with lib_types + lib_frame.

@group(1) @binding(0) var<storage, read_write> sky_camera_out: array<vec4<f32>>;

@compute @workgroup_size(1)
fn cs_sky_camera() {
    let vis = sky_map_visibility(vec3<f32>(0.0), vec3<f32>(0.0, 1.0, 0.0));
    sky_camera_out[0] = vec4<f32>(vis, 0.0, 0.0, 0.0);
}
