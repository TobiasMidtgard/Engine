// Auto-exposure metering: center-weighted histogram of log2 luminance (cd/m^2) of the
// pre-exposed HDR image. Requires lib_types only.

struct MeterParams {
    a: vec4<f32>,  // min log2 luminance, 1 / log2 range, 1 / pre-exposure, sample stride (px)
    b: vec4<u32>,  // width, height, bin count, unused
};

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var<uniform> meter: MeterParams;
@group(0) @binding(2) var<storage, read_write> bins: array<atomic<u32>>;

const MAX_BINS: u32 = 128u;
var<workgroup> local_bins: array<atomic<u32>, MAX_BINS>;

@compute @workgroup_size(16, 16, 1)
fn cs_histogram(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let count = min(meter.b.z, MAX_BINS);
    if (li < count) { atomicStore(&local_bins[li], 0u); }
    workgroupBarrier();

    let stride = u32(meter.a.w);
    let px = gid.xy * stride + vec2<u32>(stride / 2u);
    if (px.x < meter.b.x && px.y < meter.b.y) {
        let c = textureLoad(hdr, vec2<i32>(px), 0).rgb;
        let l = luminance(max(c, vec3(0.0))) * meter.a.z;
        let t = saturate((log2(max(l, 1e-12)) - meter.a.x) * meter.a.y);
        let bin = min(u32(t * f32(count)), count - 1u);
        // Center weighting: the middle of the screen counts up to 3x more than the edges.
        let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(meter.b.xy);
        let d = length((uv - 0.5) * vec2<f32>(1.0, 0.75)) * 2.0;
        let w = 1.0 + 2.0 * (1.0 - smoothstep(0.0, 0.8, d));
        atomicAdd(&local_bins[bin], u32(w * 4.0));
    }
    workgroupBarrier();
    if (li < count) {
        let v = atomicLoad(&local_bins[li]);
        if (v > 0u) { atomicAdd(&bins[li], v); }
    }
}
