// Rendering's check of the probes (`RenderSettings::ray_probe_check`, probe_gi/mod.rs
// `ProbeCheck`): the probe nearest the camera against a brute-force reference. Composed after
// probe_gi.wgsl on the frame's ray scene (lib_ray.wgsl, probe_gi_rays.wgsl), a dispatch of the
// frame's ray pass.
//
// For each of the six axis directions a surface could face, CHECK_PER_AXIS rays a frame leave
// the probe's position spread about that direction by its cosine (stratified, shifted each
// frame), and each sees what a probe ray sees (`cs_trace`): an indoor volume's wall lit by the
// probes around it, the sky through the leaves above, or a hit lit by `shade_hit` (the sun
// through a shadow ray, the lamps, last frame's probes), the back of a face black. Their mean
// radiance times pi is the irradiance a surface facing that way gets there: the reference,
// without the probes' few directions, their blend over the frames or their limit on bright
// rays. The probe's own irradiance in those directions is read from the atlas the frame's
// surfaces read, as `probe_gi` reads it.

// [0, 6): the probe's irradiance (rgb, lux) facing +x, -x, +y, -y, +z, -z, and in w its state
// (1: lit in this epoch, + 2: in use, not stuck inside something); [6]: the probe's
// camera-relative position, and in w its index in the grid (x + size.x (y + size.y z));
// [7, 7 + 6 CHECK_GROUPS): each workgroup's sum of its rays' radiance (cd/m^2, not
// pre-exposed) and in w how many rays it holds, CHECK_GROUPS a direction.
@group(2) @binding(30) var<storage, read_write> check_out: array<vec4<f32>>;

// Workgroups a direction, rays a thread: 16 x 64 x 4 = 4096 rays a direction a frame.
const CHECK_GROUPS: u32 = 16u;
const CHECK_RAYS: u32 = 4u;
const CHECK_PER_AXIS: u32 = 4096u;

var<workgroup> check_sums: array<vec4<f32>, 64>;

// The axis direction `k`: +x, -x, +y, -y, +z, -z.
fn check_axis(k: u32) -> vec3<f32> {
    let s = select(1.0, -1.0, (k & 1u) == 1u);
    let a = k / 2u;
    return vec3<f32>(select(0.0, s, a == 0u), select(0.0, s, a == 1u), select(0.0, s, a == 2u));
}

// The grid coordinate of the probe nearest the camera.
fn check_probe() -> vec3<u32> {
    let size = frame.gi_size.xyz;
    let g = clamp(round(-frame.gi_volume.xyz / frame.gi_volume.w), vec3<f32>(0.0), size - 1.0);
    return vec3<u32>(g);
}

// Ray `i` of `CHECK_PER_AXIS` about `n`, spread by the cosine: a Hammersley point shifted by
// `shift` (each frame's), mapped onto the disc and lifted onto the hemisphere.
fn check_direction(i: u32, n: vec3<f32>, shift: vec2<f32>) -> vec3<f32> {
    let u = fract(vec2<f32>((f32(i) + 0.5) / f32(CHECK_PER_AXIS), f32(reverseBits(i)) * 2.3283064365386963e-10) + shift);
    let r = sqrt(u.x);
    let phi = TAU * u.y;
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.9);
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    return normalize(t * (r * cos(phi)) + b * (r * sin(phi)) + n * sqrt(max(0.0, 1.0 - u.x)));
}

// The radiance along a ray from `origin` toward `dir`, as a probe ray sees it (`cs_trace`),
// without the probes' limit on bright rays. `cone`: the ray's share of the directions, which
// picks the level of a hit's texture.
fn check_ray(origin: vec3<f32>, dir: vec3<f32>, spacing: f32, cone: f32) -> vec3<f32> {
    let t_max = spacing * 64.0;
    let hit = trace_scene(origin, dir, t_max, false);
    let wall = sky_volume_wall(origin, dir, min(hit.t, t_max));
    if (wall < NO_HIT) {
        let p = origin + dir * wall;
        let indirect = probe_gi(p, -dir, -dir);
        return VOLUME_WALL_ALBEDO * INV_PI * indirect.rgb * indirect.a;
    }
    if (hit.instance == 0xffffffffu) {
        let through = canopy_ray_transmittance(origin, dir);
        return sample_env_rough(dir, 0.6) * through + canopy_leaf_radiance(through) * (1.0 - through);
    }
    let shaded = shade_hit(origin, dir, hit, spacing, vec2<f32>(0.0, cone));
    // (The back of a single-sided surface: the inside of something, dark.)
    return select(shaded.rgb, vec3<f32>(0.0), shaded.w < 0.0);
}

// 6 x CHECK_GROUPS workgroups: workgroup `w` traces part `w % CHECK_GROUPS` of direction
// `w / CHECK_GROUPS`; the first also reads the probe's own irradiance.
@compute @workgroup_size(64, 1, 1)
fn cs_probe_check(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) thread: u32) {
    let k = group.x / CHECK_GROUPS;
    let part = group.x % CHECK_GROUPS;
    let g = check_probe();
    let origin = probe_gi_position(vec3<f32>(g));
    let spacing = frame.gi_volume.w;
    let n = check_axis(k);
    // Each frame's shift (the R2 sequence over the frame's index).
    let frame_index = frame.time.w;
    let shift = fract(vec2<f32>(frame_index * 0.7548776662, frame_index * 0.5698402910));
    // A ray's share of the sphere's directions, as a probe's rays take theirs.
    let cone = sqrt(4.0 * PI / f32(CHECK_PER_AXIS * 2u));
    var sum = vec3<f32>(0.0);
    for (var r = 0u; r < CHECK_RAYS; r++) {
        let i = (part * 64u + thread) * CHECK_RAYS + r;
        sum += check_ray(origin, check_direction(i, n, shift), spacing, cone);
    }
    check_sums[thread] = vec4<f32>(sum, f32(CHECK_RAYS));
    // The group's sum, halving (each barrier in uniform control flow).
    workgroupBarrier();
    if (thread < 32u) { check_sums[thread] += check_sums[thread + 32u]; }
    workgroupBarrier();
    if (thread < 16u) { check_sums[thread] += check_sums[thread + 16u]; }
    workgroupBarrier();
    if (thread < 8u) { check_sums[thread] += check_sums[thread + 8u]; }
    workgroupBarrier();
    if (thread < 4u) { check_sums[thread] += check_sums[thread + 4u]; }
    workgroupBarrier();
    if (thread < 2u) { check_sums[thread] += check_sums[thread + 2u]; }
    workgroupBarrier();
    if (thread == 0u) {
        check_sums[0] += check_sums[1];
        check_out[7u + group.x] = check_sums[0];
    }
    if (group.x == 0u && thread < 7u) {
        let size = vec3<u32>(frame.gi_size.xyz);
        let c = (g + vec3<u32>(frame.gi_grid.xyz)) % size;
        let tile = probe_gi_tile(c);
        if (thread == 6u) {
            check_out[6] = vec4<f32>(origin, f32(g.x + size.x * (g.y + size.y * g.z)));
        } else {
            let atlas = vec2<f32>(textureDimensions(gi_atlas));
            let e = textureSampleLevel(gi_atlas, linear_clamp, (tile + probe_gi_dir_texel(check_axis(thread))) / atlas, 0, 0.0).rgb;
            let m = textureLoad(gi_atlas, vec2<i32>(tile), 1, 0);
            let lit = abs(m.w - frame.gi_grid.w) <= 0.5;
            let state = select(0.0, 1.0, lit) + select(0.0, 2.0, m.z >= 0.5);
            check_out[thread] = vec4<f32>(e * (PI / PROBE_GI_SCALE), state);
        }
    }
}
