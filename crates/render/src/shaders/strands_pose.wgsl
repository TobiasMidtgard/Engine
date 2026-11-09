// Strands, first pass (strands.rs): every drawn point posed by its guides, and its fibre's
// projected area added to its instance's density volume, from which the shading reads how
// much hair lies between a point and a light (as Unreal's grooms voxelize their strands for
// self-shadowing, and AMD's TressFX its fur). Requires lib_types + strands_common.

@group(0) @binding(0) var<uniform> params: StrandParams;
@group(0) @binding(1) var<storage, read> instances: array<StrandInst>;
@group(0) @binding(2) var<storage, read> rest: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> info: array<StrandInfo>;
@group(0) @binding(4) var<storage, read> guide_offsets: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> posed: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> voxels: array<atomic<u32>>;

const NONE: u32 = 0xffffffffu;

@compute @workgroup_size(256)
fn cs_pose(@builtin(global_invocation_id) id: vec3<u32>) {
    let inst = instances[id.y];
    let per = inst.first.z;
    let i = id.x;
    if (i >= inst.first.w * per) {
        return;
    }
    let s = i / per;
    let j = i - s * per;
    let at = inst.first.x + i;
    var p = rest[at];
    if (inst.out.y != NONE) {
        let f = info[inst.first.y + s].follow;
        let w01 = unpack2x16unorm(f.w);
        let w = vec3<f32>(w01, max(1.0 - w01.x - w01.y, 0.0));
        var off = vec3<f32>(0.0);
        for (var g = 0u; g < 3u; g++) {
            if (f[g] != NONE && w[g] > 0.0) {
                off += w[g] * guide_offsets[inst.out.y + f[g] * per + j].xyz;
            }
        }
        p = vec4<f32>(p.xyz + off, p.w);
    }
    posed[inst.out.x + i] = p;

    // The fibre from this point to the next, spread over the cells it passes (a long hair's
    // segments are longer than a cell).
    if (j + 1u >= per) {
        return;
    }
    let next = rest[at + 1u];
    let len = distance(next.xyz, rest[at].xyz);
    let area = 2.0 * p.w * inst.tint.w * len;
    let steps = u32(clamp(ceil(len / inst.grid.w), 1.0, 8.0));
    let d = i32(params.misc.w);
    // The next point's pose: its guides move it as they move this one, near enough.
    let ahead = next.xyz - rest[at].xyz;
    let share = u32(area / f32(steps) * STRAND_AREA_UNIT + 0.5);
    for (var k = 0u; k < steps; k++) {
        let q = p.xyz + ahead * ((f32(k) + 0.5) / f32(steps));
        let c = vec3<i32>(floor((q - inst.grid.xyz) / inst.grid.w));
        if (all(c >= vec3<i32>(0)) && all(c < vec3<i32>(d))) {
            let cell = u32(c.x + d * (c.y + d * c.z));
            atomicAdd(&voxels[inst.out.w + cell], share);
        }
    }
}
