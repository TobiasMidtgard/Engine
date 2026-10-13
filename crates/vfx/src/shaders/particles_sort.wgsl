// ============================================================================
// Particles: sort each drawn layer far to near, so nearer puffs of smoke blend over farther
// ones. Composed as lib_types + particles_common + this (no frame bindings: particles are
// stored relative to the camera, so their distance is their length).
//
// One workgroup per job, a run of up to SORT_MAX slots of one layer's ring. It writes the
// slots of the run, farthest first, to `order`, which the draw reads instance by instance.
// Keys pack a 16-bit distance (between the job's `near` and `far`) above the slot's index in
// the run; dead slots get the nearest key and padding below it, so it sorts out of the run.
// ============================================================================

struct SortJob {
    first: u32,
    count: u32,
    near: f32,
    far: f32,
};

// This frame's jobs (`count.x` of them; the buffer may hold more from earlier frames).
struct SortJobs {
    count: vec4<u32>,
    items: array<SortJob>,
};

const SORT_MAX: u32 = 2048u;
const SORT_THREADS: u32 = 256u;

@group(0) @binding(0) var<storage, read> particles: array<Particle>;
@group(0) @binding(1) var<storage, read> emitters: array<Emitter>;
@group(0) @binding(2) var<storage, read> jobs: SortJobs;
@group(0) @binding(3) var<storage, read_write> order: array<u32>;

var<workgroup> keys: array<u32, 2048>;
var<workgroup> job_count: u32;
var<workgroup> job_size: u32;

fn sort_key(job: SortJob, i: u32) -> u32 {
    if (i >= job.count) { return 0u; }
    let p = particles[job.first + i];
    var q = 1u;
    if (p.age < p.life) {
        let d = length(particle_rel(p, emitters[p.emitter]));
        q = 2u + u32(saturate((d - job.near) / max(job.far - job.near, 1e-3)) * 65533.0);
    }
    return (q << 16u) | i;
}

@compute @workgroup_size(256)
fn cs_sort(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let job_index = wg.x + wg.y * groups.x;
    if (job_index >= jobs.count.x) { return; }
    let job = jobs.items[job_index];
    if (li == 0u) {
        let count = min(job.count, SORT_MAX);
        var size = 2u;
        while (size < count) { size *= 2u; }
        job_count = count;
        job_size = size;
    }
    let count = workgroupUniformLoad(&job_count);
    let size = workgroupUniformLoad(&job_size);
    for (var i = li; i < size; i += SORT_THREADS) {
        keys[i] = sort_key(job, i);
    }
    workgroupBarrier();
    // Bitonic sort, largest key (farthest) first.
    for (var k = 2u; k <= size; k *= 2u) {
        for (var j = k / 2u; j > 0u; j /= 2u) {
            for (var t = li; t < size / 2u; t += SORT_THREADS) {
                let i = (t / j) * 2u * j + t % j;
                let a = keys[i];
                let b = keys[i + j];
                if ((a < b) == ((i & k) == 0u)) {
                    keys[i] = b;
                    keys[i + j] = a;
                }
            }
            workgroupBarrier();
        }
    }
    for (var i = li; i < count; i += SORT_THREADS) {
        order[job.first + i] = job.first + (keys[i] & 0xffffu);
    }
}
