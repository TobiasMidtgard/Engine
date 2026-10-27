// Portals (portal.rs): a portal's rectangle shows its view's picture, HDR scene color rescaled
// to this view's exposure, with no fog (the picture's own fog covers the whole way). The prepass
// writes the motion of what the picture shows, from its depth, so temporal anti-aliasing follows
// what lies beyond the portal rather than the flat rectangle.

struct PortalUniform {
    corners: array<vec4<f32>, 4>,  // camera-relative corners: (-x, -y), (+x, -y), (+x, +y), (-x, +y)
    axis_x: vec4<f32>,             // the portal's right, up and normal (toward the viewer), world
    axis_y: vec4<f32>,
    axis_z: vec4<f32>,
    tangents: vec4<f32>,           // the picture's frustum: left, right, bottom, top (x / -z, y / -z)
    params: vec4<f32>,             // the picture's near plane (m), this view's exposure / the picture's
};

@group(1) @binding(0) var<uniform> portal: PortalUniform;
@group(1) @binding(1) var picture: texture_2d<f32>;
@group(1) @binding(2) var picture_depth: texture_depth_2d;

struct PortalVertexOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_portal(@builtin(vertex_index) index: u32) -> PortalVertexOut {
    var corners = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    var uvs = array<vec2<f32>, 4>(vec2(0.0, 1.0), vec2(1.0, 1.0), vec2(1.0, 0.0), vec2(0.0, 0.0));
    let corner = corners[index];
    var out: PortalVertexOut;
    out.clip = frame.view_proj * vec4<f32>(portal.corners[corner].xyz, 1.0);
    out.uv = uvs[corner];
    return out;
}

@fragment
fn fs_portal(in: PortalVertexOut) -> @location(0) vec4<f32> {
    let color = textureSampleLevel(picture, linear_clamp, in.uv, 0.0).rgb * portal.params.y;
    return vec4<f32>(color, 1.0);
}

@fragment
fn fs_portal_prepass(in: PortalVertexOut) -> PrepassOutput {
    let size = textureDimensions(picture_depth);
    let texel = min(vec2<u32>(in.uv * vec2<f32>(size)), size - 1u);
    let depth = textureLoad(picture_depth, texel, 0);
    // What the picture shows here, relative to this camera: the picture's view looks along the
    // portal's -Z from where this camera stands (the portal's frame carries over to the exit's),
    // at view depth near / depth (reverse-Z; w = 0 is the sky, infinitely far).
    let t = portal.tangents;
    let ray = vec3<f32>(mix(t.x, t.y, in.uv.x), mix(t.w, t.z, in.uv.y), -1.0);
    let axes = mat3x3<f32>(portal.axis_x.xyz, portal.axis_y.xyz, portal.axis_z.xyz);
    let p = vec4<f32>(axes * ray, depth / portal.params.x);
    // Fixed in the world, as the portals are.
    let prev = vec4<f32>(p.xyz + frame.camera_delta.xyz * p.w, p.w);
    return prepass_output(portal.axis_z.xyz, 1.0, frame.unjittered_view_proj * p, frame.prev_view_proj * prev);
}
