// Filters for the sky environment map (environment.rs): a box-filtered mip chain of the sky,
// the sky prefiltered for GGX reflections and the cosine-convolved sky for diffuse light.
// Requires lib_types. Maps are equirectangular (`dir_to_equirect`), radiance in cd/m^2.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var env_sampler: sampler; // wraps in u, clamps in v, linear between mips
@group(0) @binding(2) var dest: texture_storage_2d<rgba16float, write>;

// Mips of the prefiltered map: perceptual roughness 0, 0.2, ... 1 (environment.rs).
const SPECULAR_MIPS: f32 = 6.0;
const PREFILTER_SAMPLES: u32 = 64u;

fn texel_center(id: vec2<u32>, size: vec2<u32>) -> vec2<f32> {
    return (vec2<f32>(id) + 0.5) / vec2<f32>(size);
}

// Next mip of the source chain: the average of 2x2 texels of the one above.
@compute @workgroup_size(8, 8)
fn cs_downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dest);
    if (id.x >= size.x || id.y >= size.y) { return; }
    let src = textureDimensions(source) - 1u;
    let p = id.xy * 2u;
    let c = textureLoad(source, min(p, src), 0) + textureLoad(source, min(p + vec2(1u, 0u), src), 0)
        + textureLoad(source, min(p + vec2(0u, 1u), src), 0) + textureLoad(source, min(p + 1u, src), 0);
    textureStore(dest, id.xy, vec4(c.rgb * 0.25, 1.0));
}

fn radical_inverse(i: u32) -> f32 {
    return f32(reverseBits(i)) * 2.3283064365386963e-10;
}

// An orthonormal basis around unit `n` (Duff et al. 2017).
fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    let t = vec3<f32>(1.0 + s * n.x * n.x * a, s * b, -s * n.x);
    let bt = vec3<f32>(b, s + n.y * n.y * a, -n.y);
    return mat3x3<f32>(t, bt, n);
}

// The sky reflected by a surface of the mip's roughness, for every reflection direction:
// GGX importance sampling with the view along the normal (Karis 2013), each sample read from the
// source mip whose texels match its share of the lobe (filtered importance sampling, Krivanek
// and Colbert 2008), so 64 samples give a smooth result. Mip 0 is the sharp sky.
@compute @workgroup_size(8, 8)
fn cs_prefilter(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dest);
    if (id.x >= size.x || id.y >= size.y) { return; }
    let src = textureDimensions(source, 0);
    let level = round(log2(f32(src.x) / f32(size.x)));
    if (level < 0.5) {
        textureStore(dest, id.xy, vec4(textureLoad(source, id.xy, 0).rgb, 1.0));
        return;
    }
    let roughness = level / (SPECULAR_MIPS - 1.0);
    let alpha = roughness * roughness;
    let n = equirect_to_dir(texel_center(id.xy, size));
    let tbn = basis(n);
    // Solid angle of a source texel at the equator; it shrinks with sin(theta) toward the poles.
    let texel_solid_angle = 2.0 * PI * PI / f32(src.x * src.y);
    let max_lod = f32(textureNumLevels(source) - 1u);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0u; i < PREFILTER_SAMPLES; i++) {
        let xi = vec2<f32>((f32(i) + 0.5) / f32(PREFILTER_SAMPLES), radical_inverse(i));
        let phi = TAU * xi.x;
        let cos_t = sqrt((1.0 - xi.y) / (1.0 + (alpha * alpha - 1.0) * xi.y));
        let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
        let h = tbn * vec3<f32>(sin_t * cos(phi), sin_t * sin(phi), cos_t);
        let l = 2.0 * cos_t * h - n;
        let n_dot_l = dot(n, l);
        if (n_dot_l <= 0.0) { continue; }
        // pdf of l = D(h) (n.h) / (4 v.h) with v = n.
        let pdf = d_ggx(cos_t, alpha) * 0.25;
        let sample_solid_angle = 1.0 / (f32(PREFILTER_SAMPLES) * pdf + 1e-4);
        let sin_theta = max(sqrt(max(1.0 - l.y * l.y, 0.0)), 0.02);
        let lod = clamp(0.5 * log2(sample_solid_angle / (texel_solid_angle * sin_theta)) + 1.0, 0.0, max_lod);
        sum += textureSampleLevel(source, env_sampler, dir_to_equirect(l), lod).rgb * n_dot_l;
        weight += n_dot_l;
    }
    textureStore(dest, id.xy, vec4(sum / max(weight, 1e-4), 1.0));
}

// Cosine-weighted mean sky radiance around every normal: irradiance / pi. Brute force over the
// 32x16 mip of the source, each texel weighted by its solid angle.
@compute @workgroup_size(8, 8)
fn cs_irradiance(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dest);
    if (id.x >= size.x || id.y >= size.y) { return; }
    let n = equirect_to_dir(texel_center(id.xy, size));
    let src0 = textureDimensions(source, 0);
    let level = u32(round(log2(f32(src0.x) / 32.0)));
    let src = textureDimensions(source, level);
    let cell = (TAU / f32(src.x)) * (PI / f32(src.y));
    var sum = vec3<f32>(0.0);
    for (var y = 0u; y < src.y; y++) {
        for (var x = 0u; x < src.x; x++) {
            let uv = texel_center(vec2(x, y), src);
            let d = equirect_to_dir(uv);
            let c = dot(n, d);
            if (c <= 0.0) { continue; }
            let solid_angle = cell * sin(uv.y * PI);
            sum += textureLoad(source, vec2(x, y), level).rgb * c * solid_angle;
        }
    }
    textureStore(dest, id.xy, vec4(sum * INV_PI, 1.0));
}
