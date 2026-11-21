//! What ray queries do on this machine's GPU (lavapipe in CI), asked of the device itself with
//! no renderer: the facts the ray scene is built on, one test each. Passes with a note where the
//! device has no ray queries.
//!
//! The scene, in meters. Rays run along -z from z = 1 unless a test says otherwise.
//! - A wall, 100 x 100, at z = -2: opaque, answers 7, class SOLID.
//! - A card, 2 x 2 around the origin at z = 0, two triangles, NOT opaque: triangle 0 is its
//!   lower right half (y < x), triangle 1 its upper left one. Answers 11, class LEAF.
//! - The same card twice as large at x = 10 (answers 12), mirrored in x at x = -10 (13), turned
//!   half around the y axis at x = 20 (14, class GROUND) and as it is at x = -20 (15, GROUND).
//! - In a second top-level structure (the "moving" one): an opaque plate of the card's size at
//!   (30, 0, -1) answering 16385, and one at (40, 0, -3) answering 16386.
//!
//! The card's vertices lie in a buffer laid out like a mesh pool page (36 bytes a vertex, the
//! position first) behind another mesh's vertices, its indices behind another mesh's; the wall's
//! like a terrain page (56 bytes a vertex, 16-bit indices).

use engine_render::gpu;
use wgpu::util::DeviceExt;

const SOLID: u32 = 1;
const GROUND: u32 = 2;
const LEAF: u32 = 8;
const ALL: u32 = 0xff;

/// No candidate is taken: a ray stops only at what is opaque.
const RULE_NONE: u32 = 0;
/// A candidate counts when it is triangle 1 of its mesh (triangle 0 is the card's "hole").
const RULE_TRIANGLE_ONE: u32 = 1;
/// A candidate counts when the query calls it a front face.
const RULE_FRONT: u32 = 2;
/// A candidate counts when the query calls it a back face.
const RULE_BACK: u32 = 3;

const FLAG_NONE: u32 = 0;
const FLAG_FORCE_OPAQUE: u32 = 1;
const FLAG_FIRST_HIT: u32 = 4;
/// `RAY_FLAG_CULL_BACK_FACING`: the ray passes every back face.
const FLAG_PASS_BACKS: u32 = 16;

/// Floats a case answers with (`put` in the shader).
const ROW: usize = 32;
const KIND: usize = 0;
const T: usize = 1;
const CUSTOM: usize = 2;
const PRIMITIVE: usize = 3;
const FRONT: usize = 4;
const BARY: usize = 5;
const TESTS: usize = 7;
const CORNERS: usize = 8;
const CORNER0_WORLD: usize = 17;
const FIRST_T: usize = 20;
const FIRST_PRIMITIVE: usize = 21;
const FIRST_CUSTOM: usize = 22;
const BACK_IN_OBJECT: usize = 24;

const SHADER: &str = r#"
enable wgpu_ray_query;
enable wgpu_ray_query_vertex_return;

struct Frame { spare: vec4<f32> }

struct Case {
    // xyz: where the ray starts, w: its far end.
    origin: vec4<f32>,
    // xyz: its direction, w: its near end.
    dir: vec4<f32>,
    // x: the rule, y: the ray's flags, z: the classes it sees, w: 1 = both structures.
    how: vec4<u32>,
}

struct Hit {
    kind: u32,
    t: f32,
    custom: u32,
    primitive: u32,
    front: f32,
    bary: vec2<f32>,
    corners: array<vec3<f32>, 3>,
    corner0_world: vec3<f32>,
    back_in_object: vec3<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var ray_still: acceleration_structure<vertex_return>;
@group(1) @binding(1) var ray_moving: acceleration_structure<vertex_return>;
@group(2) @binding(0) var<storage, read> cases: array<Case>;
@group(2) @binding(1) var<storage, read_write> results: array<f32>;

fn ray_accepts(c: RayIntersection, rule: u32, tests: ptr<function, u32>) -> bool {
    *tests += 1u;
    if rule == 1u {
        return c.primitive_index == 1u;
    }
    if rule == 2u {
        return c.front_face;
    }
    if rule == 3u {
        return !c.front_face;
    }
    return false;
}

fn first_candidate(c: RayIntersection) -> vec4<f32> {
    return vec4<f32>(c.t, f32(c.primitive_index), f32(c.instance_custom_data),
        select(0.0, 1.0, c.front_face));
}

fn nearest_still(c: Case, far_end: f32, tests: ptr<function, u32>,
    first: ptr<function, vec4<f32>>) -> Hit {
    var rq: ray_query<vertex_return>;
    rayQueryInitialize(&rq, ray_still,
        RayDesc(c.how.y, c.how.z, c.dir.w, far_end, c.origin.xyz, c.dir.xyz));
    while (rayQueryProceed(&rq)) {
        let candidate = rayQueryGetCandidateIntersection(&rq);
        if *tests == 0u {
            *first = first_candidate(candidate);
        }
        if ray_accepts(candidate, c.how.x, tests) {
            rayQueryConfirmIntersection(&rq);
        }
    }
    var hit: Hit;
    let found = rayQueryGetCommittedIntersection(&rq);
    hit.kind = found.kind;
    if found.kind == RAY_QUERY_INTERSECTION_NONE {
        return hit;
    }
    hit.t = found.t;
    hit.custom = found.instance_custom_data;
    hit.primitive = found.primitive_index;
    hit.front = select(0.0, 1.0, found.front_face);
    hit.bary = found.barycentrics;
    hit.corners = getCommittedHitVertexPositions(&rq);
    hit.corner0_world = found.object_to_world * vec4<f32>(hit.corners[0], 1.0);
    hit.back_in_object = found.world_to_object * vec4<f32>(c.origin.xyz + c.dir.xyz * found.t, 1.0);
    return hit;
}

fn nearest_moving(c: Case, far_end: f32, tests: ptr<function, u32>,
    first: ptr<function, vec4<f32>>) -> Hit {
    var rq: ray_query<vertex_return>;
    rayQueryInitialize(&rq, ray_moving,
        RayDesc(c.how.y, c.how.z, c.dir.w, far_end, c.origin.xyz, c.dir.xyz));
    while (rayQueryProceed(&rq)) {
        let candidate = rayQueryGetCandidateIntersection(&rq);
        if *tests == 0u {
            *first = first_candidate(candidate);
        }
        if ray_accepts(candidate, c.how.x, tests) {
            rayQueryConfirmIntersection(&rq);
        }
    }
    var hit: Hit;
    let found = rayQueryGetCommittedIntersection(&rq);
    hit.kind = found.kind;
    if found.kind == RAY_QUERY_INTERSECTION_NONE {
        return hit;
    }
    hit.t = found.t;
    hit.custom = found.instance_custom_data;
    hit.primitive = found.primitive_index;
    hit.front = select(0.0, 1.0, found.front_face);
    hit.bary = found.barycentrics;
    hit.corners = getCommittedHitVertexPositions(&rq);
    hit.corner0_world = found.object_to_world * vec4<f32>(hit.corners[0], 1.0);
    hit.back_in_object = found.world_to_object * vec4<f32>(c.origin.xyz + c.dir.xyz * found.t, 1.0);
    return hit;
}

fn put(i: u32, k: u32, v: f32) {
    results[i * 32u + k] = v;
}

@compute @workgroup_size(1)
fn cs_trace(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= arrayLength(&cases) {
        return;
    }
    let c = cases[i];
    var tests = 0u;
    var first = vec4<f32>(-1.0) + frame.spare;
    var hit: Hit;
    if c.how.w == 1u {
        // The moving structure first, then the still one no farther than what that found.
        let near = nearest_moving(c, c.origin.w, &tests, &first);
        var far_end = c.origin.w;
        if near.kind != RAY_QUERY_INTERSECTION_NONE {
            far_end = near.t;
        }
        let rest = nearest_still(c, far_end, &tests, &first);
        if rest.kind != RAY_QUERY_INTERSECTION_NONE {
            hit = rest;
        } else {
            hit = near;
        }
    } else {
        hit = nearest_still(c, c.origin.w, &tests, &first);
    }
    put(i, 0u, f32(hit.kind));
    put(i, 1u, hit.t);
    put(i, 2u, f32(hit.custom));
    put(i, 3u, f32(hit.primitive));
    put(i, 4u, hit.front);
    put(i, 5u, hit.bary.x);
    put(i, 6u, hit.bary.y);
    put(i, 7u, f32(tests));
    for (var k = 0u; k < 3u; k++) {
        put(i, 8u + k * 3u, hit.corners[k].x);
        put(i, 9u + k * 3u, hit.corners[k].y);
        put(i, 10u + k * 3u, hit.corners[k].z);
    }
    put(i, 17u, hit.corner0_world.x);
    put(i, 18u, hit.corner0_world.y);
    put(i, 19u, hit.corner0_world.z);
    put(i, 20u, first.x);
    put(i, 21u, first.y);
    put(i, 22u, first.z);
    put(i, 23u, first.w);
    put(i, 24u, hit.back_in_object.x);
    put(i, 25u, hit.back_in_object.y);
    put(i, 26u, hit.back_in_object.z);
}

// The measurement's dispatch: one ray, its answer added to a cell.
@compute @workgroup_size(1)
fn cs_touch(@builtin(global_invocation_id) id: vec3<u32>) {
    var rq: ray_query<vertex_return>;
    rayQueryInitialize(&rq, ray_still,
        RayDesc(RAY_FLAG_FORCE_OPAQUE, 0xffu, 0.001, 100.0, vec3<f32>(0.5, -0.5, 1.0),
            vec3<f32>(0.0, 0.0, -1.0)));
    while (rayQueryProceed(&rq)) {}
    let found = rayQueryGetCommittedIntersection(&rq);
    results[id.x] += found.t + frame.spare.x + f32(arrayLength(&cases)) * 0.0;
}
"#;

/// A cut-out card: each candidate reads its texture's alpha (`textureSampleLevel`, level 0, from
/// a layer of an array, as a leaf row reads its tile) inside the loop and is taken where that
/// is 0.5 or more. A case's `how.x` names the layer.
const CUT_OUT_SHADER: &str = r#"
enable wgpu_ray_query;
enable wgpu_ray_query_vertex_return;

struct Case {
    origin: vec4<f32>,
    dir: vec4<f32>,
    how: vec4<u32>,
}

@group(1) @binding(0) var ray_still: acceleration_structure<vertex_return>;
@group(2) @binding(0) var<storage, read> cases: array<Case>;
@group(2) @binding(1) var<storage, read_write> results: array<f32>;
@group(2) @binding(2) var cut_out: texture_2d_array<f32>;
@group(2) @binding(3) var linear_repeat: sampler;

// The card's uv at a candidate, from its triangle and weights: triangle 0 runs through corners
// 0, 1, 2 and triangle 1 through 0, 2, 3; corner (x, y) has uv ((x + 1) / 2, (y + 1) / 2).
fn card_uv(c: RayIntersection) -> vec2<f32> {
    var uv = array<vec2<f32>, 4>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0));
    let b = c.barycentrics;
    return uv[0] * (1.0 - b.x - b.y) + uv[1u + c.primitive_index] * b.x
        + uv[2u + c.primitive_index] * b.y;
}

fn put(i: u32, k: u32, v: f32) {
    results[i * 32u + k] = v;
}

@compute @workgroup_size(1)
fn cs_cut_out(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= arrayLength(&cases) {
        return;
    }
    let c = cases[i];
    var tests = 0u;
    var first = vec3<f32>(-1.0);
    var rq: ray_query<vertex_return>;
    rayQueryInitialize(&rq, ray_still,
        RayDesc(c.how.y, c.how.z, c.dir.w, c.origin.w, c.origin.xyz, c.dir.xyz));
    while (rayQueryProceed(&rq)) {
        let candidate = rayQueryGetCandidateIntersection(&rq);
        let uv = card_uv(candidate);
        let alpha = textureSampleLevel(cut_out, linear_repeat, uv, c.how.x, 0.0).a;
        if tests == 0u {
            first = vec3<f32>(alpha, uv);
        }
        tests += 1u;
        if alpha >= 0.5 {
            rayQueryConfirmIntersection(&rq);
        }
    }
    let found = rayQueryGetCommittedIntersection(&rq);
    put(i, 0u, f32(found.kind));
    put(i, 1u, found.t);
    put(i, 2u, f32(found.instance_custom_data));
    put(i, 3u, f32(found.primitive_index));
    put(i, 4u, first.x);
    put(i, 5u, first.y);
    put(i, 6u, first.z);
    put(i, 7u, f32(tests));
}
"#;

/// A fragment shader asking the same questions: the left half of the target looks through the
/// card's hole, the right half at its other triangle.
const FRAGMENT_SHADER: &str = r#"
enable wgpu_ray_query;
enable wgpu_ray_query_vertex_return;

@group(1) @binding(0) var ray_still: acceleration_structure<vertex_return>;
@group(1) @binding(1) var ray_moving: acceleration_structure<vertex_return>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    var origin = vec3<f32>(0.5, -0.5, 1.0);
    if at.x >= 2.0 {
        origin = vec3<f32>(-0.5, 0.5, 1.0);
    }
    var rq: ray_query<vertex_return>;
    rayQueryInitialize(&rq, ray_still,
        RayDesc(RAY_FLAG_NONE, 0xffu, 0.001, 100.0, origin, vec3<f32>(0.0, 0.0, -1.0)));
    while (rayQueryProceed(&rq)) {
        let candidate = rayQueryGetCandidateIntersection(&rq);
        if candidate.primitive_index == 1u {
            rayQueryConfirmIntersection(&rq);
        }
    }
    let found = rayQueryGetCommittedIntersection(&rq);
    // The moving structure too, so that both are bound in this stage.
    var other: ray_query<vertex_return>;
    rayQueryInitialize(&other, ray_moving,
        RayDesc(RAY_FLAG_NONE, 0xffu, 0.001, 100.0, vec3<f32>(30.5, -0.5, 1.0),
            vec3<f32>(0.0, 0.0, -1.0)));
    while (rayQueryProceed(&other)) {}
    let plate = rayQueryGetCommittedIntersection(&other);
    return vec4<f32>(found.t / 4.0, f32(found.instance_custom_data) / 255.0, plate.t / 4.0, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Case {
    origin: [f32; 4],
    dir: [f32; 4],
    how: [u32; 4],
}

/// A ray down -z from (`x`, `y`, 1), as far as 100 m.
fn down(x: f32, y: f32, rule: u32, flags: u32, classes: u32) -> Case {
    Case {
        origin: [x, y, 1.0, 100.0],
        dir: [0.0, 0.0, -1.0, 0.001],
        how: [rule, flags, classes, 0],
    }
}

/// The rows of a 3x4: `scale` along the axes, then moved by `at`.
fn placed(scale: [f32; 3], at: [f32; 3]) -> [f32; 12] {
    [
        scale[0], 0.0, 0.0, at[0], //
        0.0, scale[1], 0.0, at[1], //
        0.0, 0.0, scale[2], at[2],
    ]
}

struct Lab {
    gpu: gpu::Gpu,
    frame_layout: wgpu::BindGroupLayout,
    scene_layout: wgpu::BindGroupLayout,
    work_layout: wgpu::BindGroupLayout,
    frame_group: wgpu::BindGroup,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// The card's buffers and the plate's structure (the measurement fills a scene with it).
    plate: wgpu::Blas,
    still: wgpu::Tlas,
    moving: wgpu::Tlas,
    /// The structures' builds, recorded and not yet submitted: the first trace's encoder ends
    /// with the rays, as a frame's does.
    builds: Option<wgpu::CommandEncoder>,
}

fn storage(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

fn entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

const STRUCTURE_FLAGS: wgpu::AccelerationStructureFlags =
    wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE
        .union(wgpu::AccelerationStructureFlags::ALLOW_RAY_HIT_VERTEX_RETURN);

/// A mesh's triangles where they lie in shared buffers: `vertices` = (buffer, first vertex,
/// bytes a vertex), `indices` = (buffer, first index).
fn triangles<'a>(
    size: &'a wgpu::BlasTriangleGeometrySizeDescriptor,
    vertices: (&'a wgpu::Buffer, u32, u64),
    indices: (&'a wgpu::Buffer, u32),
) -> wgpu::BlasGeometries<'a> {
    wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
        size,
        vertex_buffer: vertices.0,
        first_vertex: vertices.1,
        vertex_stride: vertices.2,
        index_buffer: Some(indices.0),
        first_index: Some(indices.1),
        transform_buffer: None,
        transform_buffer_offset: None,
    }])
}

fn top_level(device: &wgpu::Device, label: &str, most: u32) -> wgpu::Tlas {
    device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some(label),
        max_instances: most,
        flags: STRUCTURE_FLAGS,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    })
}

impl Lab {
    /// The device and the scene, or `None` (with a note) where there is no GPU or it has no
    /// ray queries.
    fn new() -> Option<Lab> {
        let gpu = match gpu::create(None, 64, 64, false) {
            Ok((gpu, _)) => gpu,
            Err(e) => {
                assert!(
                    std::env::var("ENGINE_REQUIRE_GPU").as_deref() != Ok("1"),
                    "ENGINE_REQUIRE_GPU=1 but there is no GPU: {e}"
                );
                eprintln!("skipping: no GPU ({e})");
                return None;
            }
        };
        let wanted = wgpu::Features::EXPERIMENTAL_RAY_QUERY
            | wgpu::Features::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN;
        if !gpu.device.features().contains(wanted) {
            eprintln!("skipping: {} has no ray queries", gpu.info.name);
            return None;
        }
        let device = &gpu.device;

        // The card, as a mesh in a pool page: three vertices of another mesh first, 36 bytes
        // each with the position first; six indices of another mesh first.
        let mut card_vertices: Vec<f32> = Vec::new();
        let corners = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        for _ in 0..3 {
            card_vertices.extend_from_slice(&[1000.0; 9]);
        }
        for [x, y] in corners {
            card_vertices.extend_from_slice(&[x, y, 0.0, 9.0, 9.0, 9.0, 9.0, 9.0, 9.0]);
        }
        let card_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("card vertices"),
            contents: bytemuck::cast_slice(&card_vertices),
            usage: wgpu::BufferUsages::BLAS_INPUT
                | wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
        });
        let card_indices: [u32; 12] = [0, 0, 0, 0, 0, 0, 0, 1, 2, 0, 2, 3];
        let card_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("card indices"),
            contents: bytemuck::cast_slice(&card_indices),
            usage: wgpu::BufferUsages::BLAS_INPUT
                | wgpu::BufferUsages::INDEX
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
        });
        // The wall, as a terrain patch in its page: five vertices of another patch first, 56
        // bytes each; 16-bit indices behind four of another list.
        let mut wall_vertices: Vec<f32> = Vec::new();
        for _ in 0..5 {
            wall_vertices.extend_from_slice(&[-1000.0; 14]);
        }
        for [x, y] in corners {
            let mut v = [7.0f32; 14];
            v[..3].copy_from_slice(&[x * 50.0, y * 50.0, 0.0]);
            wall_vertices.extend_from_slice(&v);
        }
        let wall_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("wall vertices"),
            contents: bytemuck::cast_slice(&wall_vertices),
            usage: wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::VERTEX,
        });
        let wall_indices: [u16; 10] = [0, 0, 0, 0, 0, 1, 2, 0, 2, 3];
        let wall_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("wall indices"),
            contents: bytemuck::cast_slice(&wall_indices),
            usage: wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::INDEX,
        });

        let size = |format, flags| wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: 4,
            index_format: Some(format),
            index_count: Some(6),
            flags,
        };
        let opaque = wgpu::AccelerationStructureGeometryFlags::OPAQUE;
        let card_size = size(
            wgpu::IndexFormat::Uint32,
            wgpu::AccelerationStructureGeometryFlags::empty(),
        );
        let plate_size = size(wgpu::IndexFormat::Uint32, opaque);
        let wall_size = size(wgpu::IndexFormat::Uint16, opaque);
        let bottom_level = |label, size: &wgpu::BlasTriangleGeometrySizeDescriptor| {
            device.create_blas(
                &wgpu::CreateBlasDescriptor {
                    label: Some(label),
                    flags: STRUCTURE_FLAGS,
                    update_mode: wgpu::AccelerationStructureUpdateMode::Build,
                },
                wgpu::BlasGeometrySizeDescriptors::Triangles {
                    descriptors: vec![size.clone()],
                },
            )
        };
        let card = bottom_level("card", &card_size);
        let plate = bottom_level("plate", &plate_size);
        let wall = bottom_level("wall", &wall_size);

        let mut still = top_level(device, "still", 8);
        let mut moving = top_level(device, "moving", 4);
        let one = [1.0, 1.0, 1.0];
        let instance = |blas, to_world, answers, class: u32| {
            Some(wgpu::TlasInstance::new(
                blas,
                to_world,
                answers,
                class as u8,
            ))
        };
        still[0] = instance(&wall, placed(one, [0.0, 0.0, -2.0]), 7, SOLID);
        still[1] = instance(&card, placed(one, [0.0; 3]), 11, LEAF);
        still[2] = instance(&card, placed([2.0; 3], [10.0, 0.0, 0.0]), 12, LEAF);
        still[3] = instance(&card, placed([-1.0, 1.0, 1.0], [-10.0, 0.0, 0.0]), 13, LEAF);
        // Half a turn around y: x and z change sign.
        still[4] = instance(
            &card,
            placed([-1.0, 1.0, -1.0], [20.0, 0.0, 0.0]),
            14,
            GROUND,
        );
        still[5] = instance(&card, placed(one, [-20.0, 0.0, 0.0]), 15, GROUND);
        moving[0] = instance(&plate, placed(one, [30.0, 0.0, -1.0]), 16385, SOLID);
        moving[1] = instance(&plate, placed(one, [40.0, 0.0, -3.0]), 16386, SOLID);

        // Every bottom-level structure and both top-level ones in one call.
        let mut builds = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ray scene build"),
        });
        {
            let entries = [
                wgpu::BlasBuildEntry {
                    blas: &card,
                    geometry: triangles(
                        &card_size,
                        (&card_vertex_buffer, 3, 36),
                        (&card_index_buffer, 6),
                    ),
                },
                wgpu::BlasBuildEntry {
                    blas: &plate,
                    geometry: triangles(
                        &plate_size,
                        (&card_vertex_buffer, 3, 36),
                        (&card_index_buffer, 6),
                    ),
                },
                wgpu::BlasBuildEntry {
                    blas: &wall,
                    geometry: triangles(
                        &wall_size,
                        (&wall_vertex_buffer, 5, 56),
                        (&wall_index_buffer, 4),
                    ),
                },
            ];
            builds.build_acceleration_structures(entries.iter(), [&still, &moving]);
        }

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[entry(
                0,
                wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )],
        });
        let structure = wgpu::BindingType::AccelerationStructure {
            vertex_return: true,
        };
        let stages = wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT;
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray scene"),
            entries: &[entry(0, stages, structure), entry(1, stages, structure)],
        });
        let work_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray work"),
            entries: &[
                entry(0, wgpu::ShaderStages::COMPUTE, storage(true)),
                entry(1, wgpu::ShaderStages::COMPUTE, storage(false)),
            ],
        });
        let frame_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("frame"),
            contents: bytemuck::cast_slice(&[0.0f32; 4]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ray facts"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ray facts"),
            bind_group_layouts: &[Some(&frame_layout), Some(&scene_layout), Some(&work_layout)],
            immediate_size: 0,
        });
        Some(Lab {
            gpu,
            frame_layout,
            scene_layout,
            work_layout,
            frame_group,
            module,
            layout,
            plate,
            still,
            moving,
            builds: Some(builds),
        })
    }

    fn scene_group(&self, still: &wgpu::Tlas) -> wgpu::BindGroup {
        self.gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ray scene"),
                layout: &self.scene_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: still.as_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.moving.as_binding(),
                    },
                ],
            })
    }

    fn pipeline(&self, entry_point: &str) -> wgpu::ComputePipeline {
        self.gpu
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry_point),
                layout: Some(&self.layout),
                module: &self.module,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
    }

    fn work_group(&self, cases: &wgpu::Buffer, results: &wgpu::Buffer) -> wgpu::BindGroup {
        self.gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ray work"),
                layout: &self.work_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: cases.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: results.as_entire_binding(),
                    },
                ],
            })
    }

    fn results_buffer(&self, floats: usize) -> wgpu::Buffer {
        self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ray results"),
            size: (floats * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    fn read_floats(&self, buffer: &wgpu::Buffer) -> Vec<f32> {
        let device = &self.gpu.device;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ray results read"),
            size: buffer.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
        self.gpu.queue.submit([encoder.finish()]);
        staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let data = staging.slice(..).get_mapped_range().expect("mapped");
        data.as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    }

    fn case_buffer(&self, cases: &[Case]) -> wgpu::Buffer {
        self.gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ray cases"),
                contents: bytemuck::cast_slice(cases),
                usage: wgpu::BufferUsages::STORAGE,
            })
    }

    /// Runs `pipeline` once for each of `count` cases, over `work`, and reads `results` back as
    /// rows of `ROW` floats. The first run submits the structures' builds in its encoder, ahead
    /// of the rays, as a frame records them.
    fn run(
        &mut self,
        pipeline: &wgpu::ComputePipeline,
        work: &wgpu::BindGroup,
        results: &wgpu::Buffer,
        count: usize,
    ) -> Vec<Vec<f32>> {
        let builds = self.builds.take();
        let scene = self.scene_group(&self.still);
        let mut encoder =
            builds.unwrap_or_else(|| self.gpu.device.create_command_encoder(&Default::default()));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("rays"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_bind_group(1, &scene, &[]);
            pass.set_bind_group(2, work, &[]);
            pass.set_pipeline(pipeline);
            pass.dispatch_workgroups(count as u32, 1, 1);
        }
        self.gpu.queue.submit([encoder.finish()]);
        self.read_floats(results)
            .chunks(ROW)
            .map(|c| c.to_vec())
            .collect()
    }

    /// What each ray of `cases` finds: `ROW` floats each.
    fn trace(&mut self, cases: &[Case]) -> Vec<Vec<f32>> {
        let case_buffer = self.case_buffer(cases);
        let results = self.results_buffer(cases.len() * ROW);
        let work = self.work_group(&case_buffer, &results);
        let pipeline = self.pipeline("cs_trace");
        self.run(&pipeline, &work, &results, cases.len())
    }

    /// What each ray of `cases` finds through `CUT_OUT_SHADER`, over a texture array of two
    /// 8 x 8 layers in four cells of 4 x 4 texels. Layer 1 is clear in cells (1, 0) and (0, 1)
    /// (u, then v) and layer 0 the other way about; the RGB is white throughout.
    fn trace_cut_out(&mut self, cases: &[Case]) -> Vec<Vec<f32>> {
        let device = &self.gpu.device;
        let mut texels = Vec::new();
        for layer in 0..2u32 {
            for v in 0..8u32 {
                for u in 0..8u32 {
                    let clear = (u / 4 + v / 4 + layer) % 2 == 0;
                    texels.extend_from_slice(&[255, 255, 255, if clear { 0 } else { 255 }]);
                }
            }
        }
        let texture = device.create_texture_with_data(
            &self.gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("cut-out"),
                size: wgpu::Extent3d {
                    width: 8,
                    height: 8,
                    depth_or_array_layers: 2,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // As the rows' tiles are read: an sRGB view, whose alpha stays linear.
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &texels,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let work_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray work, cut-out"),
            entries: &[
                entry(0, wgpu::ShaderStages::COMPUTE, storage(true)),
                entry(1, wgpu::ShaderStages::COMPUTE, storage(false)),
                entry(
                    2,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                entry(
                    3,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ray facts, cut-out"),
            source: wgpu::ShaderSource::Wgsl(CUT_OUT_SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ray facts, cut-out"),
            bind_group_layouts: &[
                Some(&self.frame_layout),
                Some(&self.scene_layout),
                Some(&work_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cs_cut_out"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("cs_cut_out"),
            compilation_options: Default::default(),
            cache: None,
        });
        let case_buffer = self.case_buffer(cases);
        let results = self.results_buffer(cases.len() * ROW);
        let work = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray work, cut-out"),
            layout: &work_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: case_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: results.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        self.run(&pipeline, &work, &results, cases.len())
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// The row says the ray stopped `t` meters on, at the instance answering `answers`.
#[track_caller]
fn assert_stops(row: &[f32], t: f32, answers: u32, what: &str) {
    assert!(
        row[KIND] == 1.0 && near(row[T], t) && row[CUSTOM] == answers as f32,
        "{what}: expected a triangle of {answers} at {t} m, found kind {} of {} at {} m \
         (candidates tested: {})",
        row[KIND],
        row[CUSTOM],
        row[T],
        row[TESTS]
    );
}

/// A ray is handed every triangle that is not opaque and stops only at those its rule takes:
/// the loop a leaf card's holes and the ground's back faces need.
#[test]
fn a_ray_passes_what_its_rule_refuses_and_stops_at_what_it_takes() {
    let Some(mut lab) = Lab::new() else { return };
    let rows = lab.trace(&[
        // 0: nothing is taken: through the card, to the wall.
        down(0.5, -0.5, RULE_NONE, FLAG_NONE, ALL),
        // 1: the ray forces everything opaque: the card stops it, and nothing is asked.
        down(0.5, -0.5, RULE_NONE, FLAG_FORCE_OPAQUE, ALL),
        // 2: triangle 0 is the hole.
        down(0.5, -0.5, RULE_TRIANGLE_ONE, FLAG_NONE, ALL),
        // 3: triangle 1 is not.
        down(-0.5, 0.5, RULE_TRIANGLE_ONE, FLAG_NONE, ALL),
        // 4, 5: a first-hit ray that ends before the wall: nothing through the hole, the card
        // beside it.
        Case {
            origin: [0.5, -0.5, 1.0, 2.0],
            ..down(0.5, -0.5, RULE_TRIANGLE_ONE, FLAG_FIRST_HIT, ALL)
        },
        Case {
            origin: [-0.5, 0.5, 1.0, 2.0],
            ..down(-0.5, 0.5, RULE_TRIANGLE_ONE, FLAG_FIRST_HIT, ALL)
        },
    ]);
    assert_stops(&rows[0], 3.0, 7, "a ray that takes no candidate");
    assert_eq!(rows[0][TESTS], 1.0, "the card was offered once");
    assert!(
        near(rows[0][FIRST_T], 1.0)
            && rows[0][FIRST_PRIMITIVE] == 0.0
            && rows[0][FIRST_CUSTOM] == 11.0,
        "the candidate names its distance, triangle and instance: {:?}",
        &rows[0][FIRST_T..FIRST_T + 4]
    );
    assert_stops(&rows[1], 1.0, 11, "a ray that forces everything opaque");
    assert_eq!(rows[1][TESTS], 0.0, "a forced ray asks nothing");
    assert_eq!(rows[1][PRIMITIVE], 0.0);
    assert_stops(&rows[2], 3.0, 7, "a ray through the card's hole");
    assert_eq!(rows[2][TESTS], 1.0);
    assert_stops(&rows[3], 1.0, 11, "a ray at the card beside its hole");
    assert_eq!(rows[3][PRIMITIVE], 1.0);
    // A confirmed candidate of a triangle that is not opaque returns its corners as an opaque
    // hit does: the leaf rows' records and tiles are read from them.
    assert_eq!(
        &rows[3][CORNERS..CORNERS + 9],
        &[-1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0],
        "a confirmed candidate's corners, in index order"
    );
    assert!(
        near(rows[3][BARY], 0.25) && near(rows[3][BARY + 1], 0.5),
        "a confirmed candidate's weights of corners 1 and 2: {:?}",
        &rows[3][BARY..BARY + 2]
    );
    assert_eq!(
        rows[4][KIND], 0.0,
        "a first-hit ray through the hole that ends before the wall finds nothing"
    );
    assert_stops(&rows[5], 1.0, 11, "a first-hit ray at the card");
}

/// A candidate reads its texture inside the loop: a cut-out card stops a ray where the texel's
/// alpha is 0.5 or more and lets it through where the texel is clear, the test a leaf card's
/// holes take. The uv comes from the candidate's triangle and weights, as a record gives it.
#[test]
fn a_candidate_reads_its_texture_inside_the_loop() {
    let Some(mut lab) = Lab::new() else { return };
    // Where `CUT_OUT_SHADER` puts the first candidate's alpha and uv.
    const ALPHA: usize = 4;
    const UV: usize = 5;
    let cut = |x: f32, y: f32, layer: u32| down(x, y, layer, FLAG_NONE, ALL);
    let rows = lab.trace_cut_out(&[
        // 0, 1: triangle 0 at an opaque cell (uv 0.3, 0.2) and at a clear one (0.8, 0.3).
        cut(-0.4, -0.6, 1),
        cut(0.6, -0.4, 1),
        // 2, 3: triangle 1 at a clear cell (0.2, 0.7) and at an opaque one (0.7, 0.8).
        cut(-0.6, 0.4, 1),
        cut(0.4, 0.6, 1),
        // 4: the first ray again, reading layer 0, where that cell is clear.
        cut(-0.4, -0.6, 0),
    ]);
    assert_stops(&rows[0], 1.0, 11, "an opaque texel of triangle 0");
    assert_eq!(rows[0][PRIMITIVE], 0.0);
    assert!(
        near(rows[0][ALPHA], 1.0) && near(rows[0][UV], 0.3) && near(rows[0][UV + 1], 0.2),
        "the candidate's alpha and uv: {:?}",
        &rows[0][ALPHA..UV + 2]
    );
    assert_stops(&rows[1], 3.0, 7, "a clear texel of triangle 0");
    assert!(
        near(rows[1][ALPHA], 0.0) && near(rows[1][UV], 0.8) && near(rows[1][UV + 1], 0.3),
        "the clear texel's alpha and uv: {:?}",
        &rows[1][ALPHA..UV + 2]
    );
    assert_stops(&rows[2], 3.0, 7, "a clear texel of triangle 1");
    assert_stops(&rows[3], 1.0, 11, "an opaque texel of triangle 1");
    assert_eq!(rows[3][PRIMITIVE], 1.0);
    assert!(
        near(rows[3][UV], 0.7) && near(rows[3][UV + 1], 0.8),
        "triangle 1's uv: {:?}",
        &rows[3][UV..UV + 2]
    );
    assert_stops(
        &rows[4],
        3.0,
        7,
        "the same texel in a layer where it is clear",
    );
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row[TESTS], 1.0, "ray {i}: the card was offered once");
    }
}

/// A hit's side is told in the mesh's own space, so a mirrored instance does not swap it; and
/// a triangle whose corners run counter-clockwise as the ray sees them (the side this engine
/// draws) is a front face, as in the raster passes. A rule for "the drawn side" takes
/// `front_face`.
#[test]
fn a_hit_names_its_side_in_the_meshs_own_space() {
    let Some(mut lab) = Lab::new() else { return };
    let up = Case {
        origin: [0.5, -0.5, -1.0, 100.0],
        dir: [0.0, 0.0, 1.0, 0.001],
        how: [RULE_NONE, FLAG_FORCE_OPAQUE, LEAF, 0],
    };
    let rows = lab.trace(&[
        // 0: the card from the side its corners run counter-clockwise.
        down(0.5, -0.5, RULE_NONE, FLAG_FORCE_OPAQUE, LEAF),
        // 1: the same triangle from behind.
        up,
        // 2: the mirrored card (local x = 0.5 lies at world x = -10.5), from the same side.
        down(-10.5, -0.5, RULE_NONE, FLAG_FORCE_OPAQUE, LEAF),
        // 3, 4: the card turned half around (its back toward the ray) and the card as it is,
        // for a ray that takes only front faces.
        down(20.5, 0.25, RULE_FRONT, FLAG_NONE, ALL),
        down(-19.5, -0.5, RULE_FRONT, FLAG_NONE, ALL),
        // 5, 6: and for one that takes only back faces.
        down(20.5, 0.25, RULE_BACK, FLAG_NONE, ALL),
        down(-19.5, -0.5, RULE_BACK, FLAG_NONE, ALL),
    ]);
    assert_stops(&rows[0], 1.0, 11, "the card from its drawn side");
    assert_stops(&rows[1], 1.0, 11, "the card from behind");
    assert_stops(&rows[2], 1.0, 13, "the mirrored card");
    assert_eq!(
        rows[0][FRONT], 1.0,
        "corners counter-clockwise as the ray sees them: a front face"
    );
    assert_eq!(rows[1][FRONT], 0.0, "and the other side a back face");
    assert_eq!(
        rows[2][FRONT], rows[0][FRONT],
        "a mirrored instance keeps the side (it is told in the mesh's space)"
    );
    assert_eq!(rows[2][PRIMITIVE], 0.0);
    assert_stops(&rows[3], 3.0, 7, "a front-face ray at a card's back");
    assert_eq!(rows[3][TESTS], 1.0);
    assert_stops(&rows[4], 1.0, 15, "a front-face ray at a card's drawn side");
    assert_stops(&rows[5], 1.0, 14, "a back-face ray at a card's back");
    assert_stops(&rows[6], 3.0, 7, "a back-face ray at a card's drawn side");
}

/// A ray told to pass back faces stops only at a side this engine draws (corners
/// counter-clockwise as the ray sees them), whatever is opaque, and a mirrored instance keeps
/// its sides: what lets a shadow ray leave a closed stand-in and stops it on its way into one
/// (`ray_stopped` in lib_ray.wgsl).
#[test]
fn a_ray_told_to_pass_back_faces_stops_only_at_a_drawn_side() {
    let Some(mut lab) = Lab::new() else { return };
    let flags = FLAG_FORCE_OPAQUE | FLAG_FIRST_HIT | FLAG_PASS_BACKS;
    let rows = lab.trace(&[
        // 0: the card from its drawn side.
        down(0.5, -0.5, RULE_NONE, flags, LEAF),
        // 1: the same triangle from behind.
        Case {
            origin: [0.5, -0.5, -1.0, 100.0],
            dir: [0.0, 0.0, 1.0, 0.001],
            how: [RULE_NONE, flags, LEAF, 0],
        },
        // 2: the mirrored card from the same side as 0.
        down(-10.5, -0.5, RULE_NONE, flags, LEAF),
        // 3: the card turned half around, its back toward the ray: passed, and the wall
        // behind it stops the ray.
        down(
            20.5,
            0.25,
            RULE_NONE,
            FLAG_FORCE_OPAQUE | FLAG_PASS_BACKS,
            ALL,
        ),
    ]);
    assert_stops(&rows[0], 1.0, 11, "the card's drawn side");
    assert_eq!(rows[1][KIND], 0.0, "the card from behind is passed");
    assert_stops(&rows[2], 1.0, 13, "the mirrored card's drawn side");
    assert_stops(
        &rows[3],
        3.0,
        7,
        "a card's back is passed, the wall behind stops the ray",
    );
}

/// The corners a hit returns are the mesh's own (not the instance's), read from a vertex buffer
/// with other attributes between the positions; the hit's matrices carry them into the scene
/// and back.
#[test]
fn a_hit_returns_the_meshs_corners_and_the_way_to_the_scene() {
    let Some(mut lab) = Lab::new() else { return };
    let rows = lab.trace(&[
        // The card twice as large at x = 10: local (0.5, -0.5) lies at (11, -1).
        down(11.0, -1.0, RULE_NONE, FLAG_FORCE_OPAQUE, LEAF),
        // The wall, read from 56-byte vertices and 16-bit indices: local (3, -4).
        down(3.0, -4.0, RULE_NONE, FLAG_NONE, SOLID),
    ]);
    let row = &rows[0];
    assert_stops(row, 1.0, 12, "the scaled card");
    assert_eq!(row[PRIMITIVE], 0.0);
    assert_eq!(
        &row[CORNERS..CORNERS + 9],
        &[-1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, 0.0],
        "the corners in the mesh's space, in index order"
    );
    assert!(
        near(row[BARY], 0.5) && near(row[BARY + 1], 0.25),
        "the weights of corners 1 and 2: {:?}",
        &row[BARY..BARY + 2]
    );
    assert_eq!(
        &row[CORNER0_WORLD..CORNER0_WORLD + 3],
        &[8.0, -2.0, 0.0],
        "object_to_world places corner 0 in the scene"
    );
    assert!(
        near(row[BACK_IN_OBJECT], 0.5)
            && near(row[BACK_IN_OBJECT + 1], -0.5)
            && near(row[BACK_IN_OBJECT + 2], 0.0),
        "world_to_object brings the hit point back: {:?}",
        &row[BACK_IN_OBJECT..BACK_IN_OBJECT + 3]
    );
    let wall = &rows[1];
    assert_stops(wall, 3.0, 7, "the wall");
    assert_eq!(
        &wall[CORNERS..CORNERS + 9],
        &[-50.0, -50.0, 0.0, 50.0, -50.0, 0.0, 50.0, 50.0, 0.0],
        "the wall's corners from its page"
    );
    assert_eq!(
        &wall[CORNER0_WORLD..CORNER0_WORLD + 3],
        &[-50.0, -50.0, -2.0]
    );
}

/// An instance's class is its mask: a ray sees only the classes it names.
#[test]
fn a_ray_sees_only_the_classes_it_names() {
    let Some(mut lab) = Lab::new() else { return };
    let rows = lab.trace(&[
        down(0.5, -0.5, RULE_NONE, FLAG_FORCE_OPAQUE, SOLID),
        down(0.5, -0.5, RULE_NONE, FLAG_FORCE_OPAQUE, SOLID | LEAF),
        down(0.5, -0.5, RULE_NONE, FLAG_FORCE_OPAQUE, LEAF),
        // The ground card at x = -20 for a ray that names leaves and solids only.
        down(-19.5, -0.5, RULE_NONE, FLAG_FORCE_OPAQUE, SOLID | LEAF),
    ]);
    assert_stops(&rows[0], 3.0, 7, "a ray for solids, at a leaf card");
    assert_stops(&rows[1], 1.0, 11, "a ray for solids and leaves");
    assert_stops(&rows[2], 1.0, 11, "a ray for leaves");
    assert_stops(
        &rows[3],
        3.0,
        7,
        "a ray for solids and leaves, at a ground card",
    );
}

/// Two top-level structures are bound in one stage and built in one call; a ray asks the moving
/// one first and the still one no farther than what that found.
#[test]
fn a_ray_asks_two_structures_and_keeps_the_nearer_hit() {
    let Some(mut lab) = Lab::new() else { return };
    let both = |x: f32| Case {
        how: [RULE_NONE, FLAG_FORCE_OPAQUE, ALL, 1],
        ..down(x, -0.5, RULE_NONE, FLAG_FORCE_OPAQUE, ALL)
    };
    let rows = lab.trace(&[both(30.5), both(40.5), both(0.5)]);
    assert_stops(&rows[0], 2.0, 16385, "a plate in front of the wall");
    assert_stops(&rows[1], 3.0, 7, "the wall in front of a plate");
    assert_stops(&rows[2], 1.0, 11, "the card where nothing moves");
}

/// A fragment shader traces the same scene with the same loop (the water's mirror ray).
#[test]
fn a_fragment_shader_traces_as_a_compute_shader_does() {
    let Some(mut lab) = Lab::new() else { return };
    // The structures are built by the first trace's submission.
    lab.trace(&[down(0.5, -0.5, RULE_NONE, FLAG_NONE, ALL)]);
    let device = &lab.gpu.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ray facts, fragment"),
        source: wgpu::ShaderSource::Wgsl(FRAGMENT_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ray facts, fragment"),
        bind_group_layouts: &[Some(&lab.frame_layout), Some(&lab.scene_layout)],
        immediate_size: 0,
    });
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ray facts, fragment"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ray facts target"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let scene = lab.scene_group(&lab.still);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ray facts target read"),
        size: 256 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ray facts, fragment"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &lab.frame_group, &[]);
        pass.set_bind_group(1, &scene, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(4),
            },
        },
        wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    lab.gpu.queue.submit([encoder.finish()]);
    staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = staging.slice(..).get_mapped_range().expect("mapped");
    for y in 0..4usize {
        for x in 0..4usize {
            let px = &data[y * 256 + x * 4..y * 256 + x * 4 + 4];
            // Left: through the hole to the wall (3 m, answers 7); right: the card (1 m, 11).
            let (t, answers) = if x < 2 { (3.0, 7) } else { (1.0, 11) };
            let expected = [(t / 4.0 * 255.0f32).round() as u8, answers, 128, 255];
            assert!(
                px.iter()
                    .zip(expected)
                    .all(|(&a, b)| (a as i32 - b as i32).abs() <= 1),
                "pixel ({x}, {y}): expected {expected:?}, found {px:?}"
            );
        }
    }
}

/// A measurement, run by hand (`--ignored --nocapture`): what a submission costs the thread
/// that submits, by how its dispatches reach the ray scene. wgpu walks a structure's instances
/// at submit once for every time a group holding it is put in place: once a pass, and again
/// whenever a group before it changes. Setting the same group again is dropped, and a group
/// stays in place across pipelines that share its layout.
#[test]
#[ignore = "a measurement: prints submit times"]
fn a_pass_that_binds_the_scene_once_pays_one_walk_of_its_instances() {
    let Some(mut lab) = Lab::new() else { return };
    lab.trace(&[down(0.5, -0.5, RULE_NONE, FLAG_NONE, ALL)]);
    let device = &lab.gpu.device;
    const INSTANCES: u32 = 16_384;
    const DISPATCHES: usize = 16;
    let mut big = top_level(device, "still, full", INSTANCES);
    for i in 0..INSTANCES {
        let at = [(i % 128) as f32 * 3.0, (i / 128) as f32 * 3.0, 0.0];
        big[i as usize] = Some(wgpu::TlasInstance::new(
            &lab.plate,
            placed([1.0; 3], at),
            i,
            0xff,
        ));
    }
    let mut encoder = device.create_command_encoder(&Default::default());
    let none: [wgpu::BlasBuildEntry; 0] = [];
    encoder.build_acceleration_structures(none.iter(), [&big]);
    lab.gpu.queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");

    let scene = lab.scene_group(&big);
    let touch = lab.pipeline("cs_touch");
    let trace = lab.pipeline("cs_trace");
    let case_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ray cases"),
        contents: bytemuck::cast_slice(&[down(0.5, -0.5, RULE_NONE, FLAG_NONE, ALL)]),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let work: Vec<wgpu::BindGroup> = (0..DISPATCHES)
        .map(|_| lab.work_group(&case_buffer, &lab.results_buffer(ROW)))
        .collect();
    // A second frame group, to change the group before the scene's.
    let other_frame = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frame, other"),
        layout: &lab.frame_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("frame, other"),
                    contents: bytemuck::cast_slice(&[0.0f32; 4]),
                    usage: wgpu::BufferUsages::UNIFORM,
                })
                .as_entire_binding(),
        }],
    });
    #[derive(Clone, Copy, PartialEq)]
    enum How {
        /// One pass, the scene set once.
        Once,
        /// One pass, the scene set again before every dispatch.
        SetAgain,
        /// One pass, two pipelines of one layout in turn.
        Pipelines,
        /// One pass, the frame group before the scene's changed before every dispatch.
        FrameGroups,
        /// A pass for every dispatch.
        Passes,
    }
    let submit_ms = |how: How| {
        let mut times: Vec<f64> = (0..9)
            .map(|_| {
                let mut encoder = device.create_command_encoder(&Default::default());
                let passes = if how == How::Passes { DISPATCHES } else { 1 };
                for p in 0..passes {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("rays"),
                        timestamp_writes: None,
                    });
                    pass.set_bind_group(0, &lab.frame_group, &[]);
                    pass.set_bind_group(1, &scene, &[]);
                    pass.set_pipeline(&touch);
                    let dispatches = if how == How::Passes {
                        p..p + 1
                    } else {
                        0..DISPATCHES
                    };
                    for i in dispatches {
                        match how {
                            How::SetAgain => pass.set_bind_group(1, &scene, &[]),
                            How::Pipelines => {
                                pass.set_pipeline(if i % 2 == 0 { &touch } else { &trace })
                            }
                            How::FrameGroups => pass.set_bind_group(
                                0,
                                if i % 2 == 0 {
                                    &other_frame
                                } else {
                                    &lab.frame_group
                                },
                                &[],
                            ),
                            How::Once | How::Passes => {}
                        }
                        pass.set_bind_group(2, &work[i], &[]);
                        pass.dispatch_workgroups(1, 1, 1);
                    }
                }
                let commands = encoder.finish();
                let started = std::time::Instant::now();
                lab.gpu.queue.submit([commands]);
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("poll");
                ms
            })
            .collect();
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    let once = submit_ms(How::Once);
    let set_again = submit_ms(How::SetAgain);
    let pipelines = submit_ms(How::Pipelines);
    let frame_groups = submit_ms(How::FrameGroups);
    let passes = submit_ms(How::Passes);
    eprintln!(
        "{INSTANCES} instances, {DISPATCHES} dispatches, median submit of 9: one pass with the \
         scene set once {once:.3} ms; set again before every dispatch {set_again:.3} ms; two \
         pipelines in turn {pipelines:.3} ms; the frame group changed before every dispatch \
         {frame_groups:.3} ms; a pass for every dispatch {passes:.3} ms"
    );
    for (name, ms) in [("set again", set_again), ("two pipelines", pipelines)] {
        assert!(
            ms < once * 2.0 + 0.2,
            "{name}: the scene should not be put in place again ({ms:.3} ms against \
             {once:.3} ms)"
        );
    }
    for (name, ms) in [("frame groups", frame_groups), ("passes", passes)] {
        assert!(
            ms > once * 3.0,
            "{name}: the scene is put in place before every dispatch, which should cost \
             several times one pass that binds it once ({ms:.3} ms against {once:.3} ms)"
        );
    }
}
