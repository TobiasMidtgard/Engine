// ============================================================================
// Procedural material generator: bindings and passes (material_library.rs).
// Composed after material_gen_lib.wgsl and one preset, which defines:
//
//   fn preset_layers(uv) -> Layers        the shapes: height (m) in `a.x`, masks and ids in the rest
//   fn preset_refine(uv) -> Layers        an optional second look at them (slope blur, erosion,
//                                         water filling hollows); reads `layers_at`
//   fn preset_surface(uv, Layers, Derived) -> Pbr   colors, roughness and metalness from the
//                                         shapes and what the height field tells (normal,
//                                         ambient occlusion, curvature)
//
// Passes: layers → refine (0+ times, ping-pong) → derive (normal, AO, curvature from the height)
// → surface (writes the four 8-bit maps) → mips (material_gen_mips.wgsl).
// ============================================================================

struct GenParams {
    size: vec4<u32>,  // texels per side, seed, refine iteration, unused
    dims: vec4<f32>,  // tile size (m), relief depth (m), ambient occlusion radius (m), curvature radius (m)
};

struct Layers {
    a: vec4<f32>,  // x: height (m); yzw preset-defined
    b: vec4<f32>,  // preset-defined
};

struct Derived {
    normal: vec3<f32>,  // tangent space (+x right, +y up in the image)
    ao: f32,            // ambient occlusion from the height field (1 = open)
    curv_fine: f32,     // > 0 in hollows, < 0 on edges, at the curvature radius (about [-1, 1])
    curv_broad: f32,    // the same at the ambient occlusion radius
    slope: f32,         // steepness (0 flat .. 1 at 45 degrees and beyond)
};

struct Pbr {
    albedo: vec3<f32>,   // linear base color
    roughness: f32,      // perceptual
    metallic: f32,
    ao: f32,             // multiplied with the height-field occlusion
    normal: vec3<f32>,   // tangent-space normal (starts as the derived one)
};

@group(0) @binding(0) var<uniform> gen: GenParams;
@group(0) @binding(1) var src_a: texture_2d<f32>;
@group(0) @binding(2) var src_b: texture_2d<f32>;
@group(0) @binding(3) var src_c: texture_2d<f32>;
@group(0) @binding(4) var src_d: texture_2d<f32>;
@group(0) @binding(5) var dst_a: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var dst_b: texture_storage_2d<rgba32float, write>;
@group(0) @binding(7) var out_albedo: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(8) var out_normal: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(9) var out_orm: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(10) var out_height: texture_storage_2d<rgba8unorm, write>;
// [0]: texels with a non-finite value, [1]: texels whose albedo was clamped.
@group(0) @binding(11) var<storage, read_write> stats: array<atomic<u32>, 4>;

fn texels() -> i32 {
    return i32(gen.size.x);
}

// Size of one texel in meters.
fn texel_m() -> f32 {
    return gen.dims.x / f32(gen.size.x);
}

fn wrap_texel(p: vec2<i32>) -> vec2<i32> {
    let n = texels();
    return ((p % n) + n) % n;
}

// Bilinear read of an rgba32float source at tile coordinates `uv` (wraps around).
fn bilinear(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let p = uv * f32(texels()) - 0.5;
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let a = textureLoad(t, wrap_texel(i), 0);
    let b = textureLoad(t, wrap_texel(i + vec2<i32>(1, 0)), 0);
    let c = textureLoad(t, wrap_texel(i + vec2<i32>(0, 1)), 0);
    let d = textureLoad(t, wrap_texel(i + vec2<i32>(1, 1)), 0);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// The layers of the previous pass at `uv` (for `preset_refine`).
fn layers_at(uv: vec2<f32>) -> Layers {
    return Layers(bilinear(src_a, uv), bilinear(src_b, uv));
}

// Height (m) of the previous pass at `uv` (for `preset_refine`).
fn height_at(uv: vec2<f32>) -> f32 {
    return bilinear(src_a, uv).x;
}

// Gradient of the previous pass's height at `uv`, meters per meter (+x right, +y down the image).
fn height_gradient(uv: vec2<f32>) -> vec2<f32> {
    let e = 1.0 / f32(texels());
    let gx = height_at(uv + vec2<f32>(e, 0.0)) - height_at(uv - vec2<f32>(e, 0.0));
    let gy = height_at(uv + vec2<f32>(0.0, e)) - height_at(uv - vec2<f32>(0.0, e));
    return vec2<f32>(gx, gy) / (2.0 * texel_m());
}

// Slope blur (as in Substance Designer): averages the previous height along its own uphill
// direction over up to `distance` meters (less on gentle slopes), in `steps` samples weighted
// toward the start. Steep detail smears down the slopes like weathering; flat areas stay.
fn slope_blur(uv: vec2<f32>, distance: f32, steps: i32) -> f32 {
    let g = height_gradient(uv);
    let len = length(g);
    if (len < 1e-4) {
        return height_at(uv);
    }
    let dir = g / len * (distance / gen.dims.x) * min(len, 1.0);
    var sum = 0.0;
    var weight = 0.0;
    for (var i = 0; i < steps; i++) {
        let t = f32(i) / f32(max(steps - 1, 1));
        let w = 1.0 - 0.6 * t;
        sum += height_at(uv + dir * t) * w;
        weight += w;
    }
    return sum / weight;
}

fn texel_uv(id: vec2<u32>) -> vec2<f32> {
    return (vec2<f32>(id) + 0.5) / f32(gen.size.x);
}

@compute @workgroup_size(8, 8)
fn cs_layers(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= gen.size.x || id.y >= gen.size.x) { return; }
    let l = preset_layers(texel_uv(id.xy));
    textureStore(dst_a, id.xy, l.a);
    textureStore(dst_b, id.xy, l.b);
}

@compute @workgroup_size(8, 8)
fn cs_refine(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= gen.size.x || id.y >= gen.size.x) { return; }
    let l = preset_refine(texel_uv(id.xy));
    textureStore(dst_a, id.xy, l.a);
    textureStore(dst_b, id.xy, l.b);
}

fn h_texel(p: vec2<i32>) -> f32 {
    return textureLoad(src_a, wrap_texel(p), 0).x;
}

fn h_bilinear(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    return mix(mix(h_texel(i), h_texel(i + vec2<i32>(1, 0)), f.x),
               mix(h_texel(i + vec2<i32>(0, 1)), h_texel(i + vec2<i32>(1, 1)), f.x), f.y);
}

// Mean height on a ring of `r` texels around `p` minus the height at `p`, over the radius:
// positive in hollows, negative on ridges and edges (dimensionless, like a slope).
fn ring_curvature(p: vec2<i32>, h0: f32, r: f32) -> f32 {
    var sum = 0.0;
    for (var k = 0; k < 8; k++) {
        let a = f32(k) * (TAU / 8.0) + 0.3927;
        sum += h_bilinear(vec2<f32>(p) + vec2<f32>(cos(a), sin(a)) * r);
    }
    return (sum / 8.0 - h0) / (r * texel_m());
}

const AO_DIRECTIONS: i32 = 12;
const AO_STEPS: i32 = 8;

// Normal (Sobel), ambient occlusion (horizon search over the height field) and curvature.
@compute @workgroup_size(8, 8)
fn cs_derive(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= gen.size.x || id.y >= gen.size.x) { return; }
    let p = vec2<i32>(id.xy);
    let t = texel_m();
    let tl = h_texel(p + vec2<i32>(-1, -1));
    let tc = h_texel(p + vec2<i32>(0, -1));
    let tr = h_texel(p + vec2<i32>(1, -1));
    let ml = h_texel(p + vec2<i32>(-1, 0));
    let h0 = h_texel(p);
    let mr = h_texel(p + vec2<i32>(1, 0));
    let bl = h_texel(p + vec2<i32>(-1, 1));
    let bc = h_texel(p + vec2<i32>(0, 1));
    let br = h_texel(p + vec2<i32>(1, 1));
    // Slopes in meters per meter; rows grow downward, the normal's +y points up.
    let dx = ((tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl)) / (8.0 * t);
    let dup = ((tl + 2.0 * tc + tr) - (bl + 2.0 * bc + br)) / (8.0 * t);
    let normal = normalize(vec3<f32>(-dx, -dup, 1.0));

    let radius = max(gen.dims.z / t, 2.0);
    var ao = 0.0;
    for (var d = 0; d < AO_DIRECTIONS; d++) {
        let a = (f32(d) + 0.5) * (TAU / f32(AO_DIRECTIONS));
        let dir = vec2<f32>(cos(a), sin(a));
        var max_tan = 0.0;
        for (var s = 0; s < AO_STEPS; s++) {
            let k = f32(s) / f32(AO_STEPS - 1);
            let r = 1.0 + (radius - 1.0) * k * k;
            let h = h_bilinear(vec2<f32>(p) + dir * r);
            max_tan = max(max_tan, (h - h0) / (r * t));
        }
        // Cosine-weighted share of this slice of sky above the horizon.
        ao += 1.0 / (1.0 + max_tan * max_tan);
    }
    ao /= f32(AO_DIRECTIONS);

    let fine = ring_curvature(p, h0, max(gen.dims.w / t, 1.0));
    let broad = ring_curvature(p, h0, radius * 0.5);
    let slope = saturate(length(vec2<f32>(dx, dup)));
    textureStore(dst_a, id.xy, vec4<f32>(normal, ao));
    textureStore(dst_b, id.xy, vec4<f32>(fine, broad, slope, 0.0));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = saturate3(c);
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}

fn finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

fn finite4(v: vec4<f32>) -> bool {
    return finite(v.x) && finite(v.y) && finite(v.z) && finite(v.w);
}

@compute @workgroup_size(8, 8)
fn cs_surface(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= gen.size.x || id.y >= gen.size.x) { return; }
    let p = vec2<i32>(id.xy);
    let l = Layers(textureLoad(src_a, p, 0), textureLoad(src_b, p, 0));
    let da = textureLoad(src_c, p, 0);
    let db = textureLoad(src_d, p, 0);
    let d = Derived(da.xyz, da.w, db.x, db.y, db.z);
    var s = preset_surface(texel_uv(id.xy), l, d);
    let height = l.a.x / gen.dims.y;
    let ok = finite4(vec4<f32>(s.albedo, s.roughness)) && finite4(vec4<f32>(s.normal, s.metallic))
        && finite(s.ao) && finite(height) && finite4(l.a) && finite4(l.b) && finite4(da) && finite4(db);
    if (!ok) {
        atomicAdd(&stats[0], 1u);
    }
    if (any(s.albedo > vec3<f32>(1.0)) || any(s.albedo < vec3<f32>(0.0))) {
        atomicAdd(&stats[1], 1u);
    }
    let n = normalize(select(vec3<f32>(0.0, 0.0, 1.0), s.normal, length(s.normal) > 1e-6));
    textureStore(out_albedo, p, vec4<f32>(linear_to_srgb(s.albedo), 1.0));
    textureStore(out_normal, p, vec4<f32>(n * 0.5 + 0.5, 1.0));
    textureStore(out_orm, p, vec4<f32>(saturate(s.ao * d.ao), clamp(s.roughness, 0.02, 1.0), saturate(s.metallic), 1.0));
    let h01 = saturate(height);
    textureStore(out_height, p, vec4<f32>(h01, h01, h01, 1.0));
}
