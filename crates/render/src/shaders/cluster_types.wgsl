// Virtual geometry data shared by the culling and drawing shaders (virtual_geometry/gpu.rs).

// One instance of a clustered mesh, camera-relative. Matches `GpuInstance`.
struct ClusterInstance {
    m0: vec4<f32>,      // model matrix rows (camera-relative)
    m1: vec4<f32>,
    m2: vec4<f32>,
    n0: vec4<f32>,      // normal matrix columns
    n1: vec4<f32>,
    n2: vec4<f32>,
    p0: vec4<f32>,      // last frame's model matrix rows, relative to last frame's camera
    p1: vec4<f32>,
    p2: vec4<f32>,
    extra: vec4<u32>,   // material index, flags (INSTANCE_*), mesh min y (f32 bits), mesh height (f32 bits)
    sphere: vec4<f32>,  // camera-relative sphere around all its clusters and error spheres (m)
    mesh: vec4<u32>,    // first level, level count, bucket (texture set and sidedness) in the camera view, unused
    lod: vec4<f32>,     // largest axis scale (errors and radii grow by it), normal cone stretch (1: rotation and uniform scale), unused, unused
};

// The transform mirrors the mesh: two corners of every triangle are swapped so front faces stay
// counter-clockwise.
const INSTANCE_MIRRORED: u32 = 1u;
// Single-sided, rotated and scaled without shear: normal cones are valid for backface culling
// (widened by the instance's stretch).
const INSTANCE_CONE_CULL: u32 = 2u;
const INSTANCE_CASTS_SHADOWS: u32 = 4u;

// One cluster: up to 128 triangles of one level of detail. Matches `GpuCluster`.
struct Cluster {
    bounds: vec4<f32>,         // sphere around its triangles (mesh space, m)
    lod_bounds: vec4<f32>,     // sphere its error is measured over
    parent_bounds: vec4<f32>,  // the same for the coarser clusters made from its group
    cone: vec4<f32>,           // normal cone axis, cosine of its half angle (<= 0: no cone)
    errors: vec4<f32>,         // own error, parent error (m), unused, unused
    data: vec4<u32>,           // first index, first vertex, vertex count | triangle count << 16, level
};

// Vertices drawn per visible cluster: three per triangle of a full cluster (the corners of
// missing triangles collapse to a point and draw nothing).
const CLUSTER_VERTEX_COUNT: u32 = 384u;
