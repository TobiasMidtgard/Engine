// Screen-space subsurface scattering (subsurface.rs): Jimenez et al.'s separable filter. Two
// passes, along x then y, blur the diffuse light of the pixels the prepass marked
// (`prepass_output_subsurface`) with the skin's diffusion profile, red reaching farthest. The
// scene pass leaves those pixels' specular light in alpha, so highlights stay sharp. The second
// pass multiplies the blurred light by the post-scatter color the features drew (white where
// none did). Requires lib_types.

const SSS_SAMPLES: u32 = 17u;

struct SssParams {
    // Per sample: weight per channel (rgb), offset in units of the profile's reach (-1..1).
    // Sample 0 is the center.
    kernel: array<vec4<f32>, 17>,
    // x: pixels per meter at 1 m from the camera, y: near plane (m, reverse-Z depth = near /
    // view depth), z: the profile's reach at full strength (m), w unused.
    params: vec4<f32>,
    // xy: direction in pixels, z: 1 on the first pass (alpha holds specular light), w unused.
    dir: vec4<f32>,
};

@group(0) @binding(0) var<uniform> sss: SssParams;
@group(0) @binding(1) var sss_sampler: sampler;
@group(0) @binding(2) var sss_color: texture_2d<f32>;
@group(0) @binding(3) var sss_depth: texture_depth_2d;
@group(0) @binding(4) var sss_normal: texture_2d<f32>;
@group(0) @binding(5) var sss_post_color: texture_2d<f32>;

@vertex
fn vs_subsurface(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

// How strongly a pixel scatters (0 = not marked).
fn sss_strength(px: vec2<i32>) -> f32 {
    return saturate(textureLoad(sss_normal, px, 0).w - 1.0);
}

fn sss_view_depth(px: vec2<i32>) -> f32 {
    return sss.params.y / max(textureLoad(sss_depth, px, 0), 1e-9);
}

// A sample's diffuse light: on the first pass the scene color less the specular in alpha.
fn sss_diffuse(c: vec4<f32>, first: bool) -> vec3<f32> {
    return select(c.rgb, max(c.rgb - vec3<f32>(c.a), vec3<f32>(0.0)), first);
}

@fragment
fn fs_subsurface(in: FullscreenOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(sss_color));
    let px = clamp(vec2<i32>(in.position.xy), vec2<i32>(0), size - 1);
    let strength = sss_strength(px);
    if (strength <= 0.0) {
        discard;
    }
    let first = sss.dir.z > 0.5;
    let center = textureLoad(sss_color, px, 0);
    let specular = center.a;
    let diffuse = sss_diffuse(center, first);
    let depth = sss_view_depth(px);
    let width = sss.params.z * strength;
    // The profile's reach on screen, pixels.
    let reach = width * sss.params.x / depth;
    var color = diffuse;
    if (reach >= 0.5) {
        color = diffuse * sss.kernel[0].rgb;
        let texel = 1.0 / vec2<f32>(size);
        for (var i = 1u; i < SSS_SAMPLES; i++) {
            let k = sss.kernel[i];
            let offset = sss.dir.xy * (k.w * reach);
            let q = clamp(px + vec2<i32>(round(offset)), vec2<i32>(0), size - 1);
            var s = diffuse;
            // Light scatters within the marked surface only, and not across a step in depth
            // (an ear's rim over the cheek behind it): such samples count as the center.
            if (sss_strength(q) > 0.0) {
                let uv = (vec2<f32>(px) + 0.5 + offset) * texel;
                let c = textureSampleLevel(sss_color, sss_sampler, uv, 0.0);
                let step = abs(sss_view_depth(q) - depth);
                s = mix(sss_diffuse(c, first), diffuse, saturate(step / max(width, 1e-4) - 0.5));
            }
            color += k.rgb * s;
        }
    }
    if (first) {
        return vec4<f32>(color, specular);
    }
    let post = textureLoad(sss_post_color, px, 0).rgb;
    return vec4<f32>(color * post + vec3<f32>(specular), 1.0);
}
