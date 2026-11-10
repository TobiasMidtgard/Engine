// Meshes lit once a pixel (`trial_cut::MESHES_ONCE`, mesh_feature.rs "once"): mesh.wgsl's
// `fs_once` stored each pixel's material in the meshes' own prepass; one triangle over the
// screen lights it here, in the scene pass, where the scene pass would have drawn the meshes
// again and run `fs_main` for every triangle that touches the pixel's 2x2 block.

@group(1) @binding(0) var once_albedo: texture_2d<f32>;
@group(1) @binding(1) var once_emissive: texture_2d<f32>;
@group(1) @binding(2) var once_normals: texture_2d<f32>;
@group(1) @binding(3) var once_surface: texture_2d<f32>;
@group(1) @binding(4) var once_depth: texture_2d<f32>;

struct OnceLit {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// (Invariant: the pipeline tests depth for equality, though the fragment gives its own.)
struct OnceCorner {
    @builtin(position) @invariant position: vec4<f32>,
};

@vertex
fn vs_once_light(@builtin(vertex_index) i: u32) -> OnceCorner {
    var out: OnceCorner;
    out.position = fullscreen_vertex(i, 0.0).position;
    return out;
}

@fragment
fn fs_once_light(@builtin(position) at: vec4<f32>) -> OnceLit {
    let pixel = vec2<i32>(at.xy);
    let stored = textureLoad(once_depth, pixel, 0).xy;
    let z = textureLoad(prepass_depth, pixel, 0);
    // Only where the stored mesh is what the prepass left in view (something drawn into the
    // prepass afterwards may stand in front of it). The depth the shader saw and the depth the
    // rasterizer wrote may differ by rounding.
    if (stored.y < 0.5 || abs(bitcast<i32>(stored.x) - bitcast<i32>(z)) > 2) {
        discard;
    }
    var out: OnceLit;
    // Tested for equality against the scene's depth: whatever has been drawn in front since
    // the prepass (hair) keeps its pixels.
    out.depth = z;
    if (shade_skip(SKIP_SHOW_MESHES_ONCE)) {
        out.color = vec4<f32>(1.0, 0.0, 1.0, 1.0);
        return out;
    }
    let uv = at.xy * frame.viewport.zw;
    let rel = (frame.inv_view * vec4<f32>(view_position(uv, z), 0.0)).xyz;
    let basic = textureLoad(prepass_normal, pixel, 0);
    let albedo = textureLoad(once_albedo, pixel, 0);
    let emissive = textureLoad(once_emissive, pixel, 0);
    let normals = textureLoad(once_normals, pixel, 0);
    let surface = textureLoad(once_surface, pixel, 0);

    var s = default_surface();
    s.albedo = albedo.rgb;
    s.metallic = surface.x;
    s.roughness = basic.z;
    s.reflectance = surface.z;
    s.normal = oct_decode(basic.xy);
    // (Stored pre-exposed and added below: its radiance may be past what the target holds.)
    s.emissive = vec3<f32>(0.0);
    s.occlusion = albedo.a;
    s.subsurface = surface.y;
    s.smooth_normal = oct_decode(normals.xy);

    var color: vec3<f32>;
    if (stored.y > 2.5) {
        color = s.albedo;
    } else if (stored.y > 1.5) {
        // Leaves, petals, cloth: lit from behind they glow by `subsurface`, never shine.
        color = shade_thin_surface(s, rel, emissive.a, oct_decode(normals.zw));
    } else {
        color = shade_surface_shadowed(s, rel, emissive.a);
    }
    color += emissive.rgb;
    out.color = vec4<f32>(apply_fog(color, rel), surface.w);
    return out;
}
