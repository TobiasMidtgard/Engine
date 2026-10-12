// Post-processing: tonemapping (HDR pre-exposed -> display LDR) with bloom and grading.
// Requires lib_types only.

struct PostParams {
    // x: tonemapper (0 AgX, 1 ACES, 2 Reinhard, 3 none), y: saturation, z: contrast, w: vignette
    params: vec4<f32>,
    // x: bloom intensity (0..1 mix), y: film grain, z: real time, w: bloom normalization
    params2: vec4<f32>,
    // x: sharpening 0..1, y: white balance warm(+)/cool(-), z: pre-exposure, w: night desaturation
    params3: vec4<f32>,
};

@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var post_sampler: sampler;
@group(0) @binding(2) var<uniform> post: PostParams;
@group(0) @binding(3) var bloom_tex: texture_2d<f32>;

fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

fn agx(c: vec3<f32>) -> vec3<f32> {
    let agx_mat = mat3x3<f32>(
        vec3<f32>(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3<f32>(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3<f32>(0.0792237451477643, 0.0791661274605434, 0.879142973793104));
    let inv_mat = mat3x3<f32>(
        vec3<f32>(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3<f32>(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3<f32>(-0.0990297440797205, -0.0989611768448433, 1.15107367264116));
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var v = agx_mat * max(c, vec3(0.0));
    v = clamp(log2(max(v, vec3(1e-10))), vec3(min_ev), vec3(max_ev));
    v = (v - min_ev) / (max_ev - min_ev);
    v = agx_contrast(v);
    v = inv_mat * v;
    return pow(max(v, vec3(0.0)), vec3(2.2));
}

fn aces_fitted(c: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let cc = 2.43;
    let d = 0.59;
    let e = 0.14;
    return saturate3((c * (a * c + b)) / (c * (cc * c + d) + e));
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

// Contrast-adaptive sharpening (after AMD FidelityFX CAS): pushes each pixel away from its four
// neighbors, less where the neighborhood is already contrasty, and never beyond the
// neighborhood's range, so temporal anti-aliasing's softness goes without halos. Works on
// compressed values (c / (1 + c)) so bright edges don't ring.
fn sharpened(uv: vec2<f32>, amount: f32) -> vec3<f32> {
    let px = vec2<i32>(uv * vec2<f32>(textureDimensions(hdr_tex)));
    let size = vec2<i32>(textureDimensions(hdr_tex)) - 1;
    let c = textureLoad(hdr_tex, px, 0).rgb;
    if (amount <= 0.0) { return c; }
    let n = textureLoad(hdr_tex, clamp(px + vec2<i32>(0, -1), vec2<i32>(0), size), 0).rgb;
    let s = textureLoad(hdr_tex, clamp(px + vec2<i32>(0, 1), vec2<i32>(0), size), 0).rgb;
    let e = textureLoad(hdr_tex, clamp(px + vec2<i32>(1, 0), vec2<i32>(0), size), 0).rgb;
    let w = textureLoad(hdr_tex, clamp(px + vec2<i32>(-1, 0), vec2<i32>(0), size), 0).rgb;
    let cc = c / (1.0 + c);
    let nc = n / (1.0 + n);
    let sc = s / (1.0 + s);
    let ec = e / (1.0 + e);
    let wc = w / (1.0 + w);
    let lo = min(cc, min(min(nc, sc), min(ec, wc)));
    let hi = max(cc, max(max(nc, sc), max(ec, wc)));
    // Less sharpening where the local contrast is already high.
    let headroom = saturate3(min(lo, 1.0 - hi) / max(hi, vec3<f32>(1e-4)));
    let k = -sqrt(headroom) * mix(0.125, 0.2, amount);
    let r = clamp((cc + (nc + sc + ec + wc) * k) / (1.0 + 4.0 * k), lo, hi);
    return r / max(1.0 - r, vec3<f32>(1e-4));
}

@fragment
fn fs_tonemap(in: FullscreenOut) -> @location(0) vec4<f32> {
    var c = sharpened(in.uv, post.params3.x);
    let bloom = textureSampleLevel(bloom_tex, post_sampler, in.uv, 0.0).rgb * post.params2.w;
    c = mix(c, bloom, post.params2.x);
    // Scotopic (night) vision: below ~1 cd/m^2 color fades toward a blue-gray.
    let night = post.params3.w;
    if (night > 0.0) {
        let l_abs = luminance(c) / max(post.params3.z, 1e-12);
        let scotopic = smoothstep(1.0, 0.005, l_abs) * night;
        c = mix(c, vec3(luminance(c)) * vec3(0.75, 0.9, 1.25), scotopic);
    }
    // White balance: simple warm/cool tint that preserves luminance.
    let wb = post.params3.y;
    if (wb != 0.0) {
        let tint = vec3<f32>(1.0 + 0.1 * wb, 1.0, 1.0 - 0.1 * wb);
        c = c * tint * (luminance(c) / max(luminance(c * tint), 1e-6));
    }
    let mode = u32(post.params.x);
    var mapped: vec3<f32>;
    if (mode == 0u) {
        mapped = agx(c);
    } else if (mode == 1u) {
        mapped = aces_fitted(c * 0.8);
    } else if (mode == 2u) {
        mapped = c / (1.0 + c);
    } else {
        mapped = saturate3(c);
    }
    // Saturation & contrast grading.
    let l = luminance(mapped);
    mapped = mix(vec3(l), mapped, post.params.y);
    mapped = saturate3((mapped - 0.18) * post.params.z + 0.18);
    // Vignette.
    let d = distance(in.uv, vec2(0.5));
    mapped *= 1.0 - post.params.w * smoothstep(0.4, 0.85, d);
    // Film grain.
    let grain = (hash12(in.uv * 1000.0 + post.params2.z) - 0.5) * post.params2.y;
    mapped = max(mapped + grain, vec3(0.0));
    // Dither by one 8-bit step in display (sRGB) space so skies and fog don't band.
    let px = in.uv * 4096.0;
    let dither = (hash12(px + fract(post.params2.z) * 61.0) + hash12(px.yx + 17.3) - 1.0) / 255.0;
    let encoded = pow(mapped, vec3(1.0 / 2.2)) + dither;
    mapped = pow(max(encoded, vec3(0.0)), vec3(2.2));
    return vec4<f32>(mapped, 1.0);
}

