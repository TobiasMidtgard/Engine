// ray_water.wgsl's `ray_reflection` where the frame's ray scene isn't bound (a device without ray
// queries, a frame or a view without the scene, the water's pipelines that don't trace): it finds
// nothing. No binding, no `enable`; change the two together.
fn ray_reflection(rel: vec3<f32>, r: vec3<f32>, rough: f32) -> vec4<f32> {
    return vec4<f32>(0.0);
}
