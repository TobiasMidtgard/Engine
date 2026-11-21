// The ray scene (ray_scene/mod.rs): what the frame's rays trace on ray-tracing hardware. Two
// acceleration structures, one of the instances that hold still and one of those that move,
// and a row for each instance that a hit reads. Composed before the shader that traces; the
// scene is group 1 of its pipeline, set once in the frame's one ray pass.
//
// Positions are camera-relative, as everywhere; the structures' instances stand around an
// anchor (`ray_scene.anchor`, camera-relative) that moves only when the camera is far from it,
// so the structures stay as built while the camera moves.
enable wgpu_ray_query;
enable wgpu_ray_query_vertex_return;

struct RayUniform {
    // the anchor, camera-relative
    anchor: vec4<f32>,
    // instances in the still structure, in the moving one; the frame; the epoch
    counts: vec4<u32>,
    // shadow stand-ins in the still structure, in the moving one
    stand_ins: vec4<u32>,
    // the sun's rays: reach of the pixels that get one, length (meters); zeros until the
    // sun's own rays read them (the measuring rays have a length of their own)
    sun: vec4<f32>,
    // caster reach, far reach (meters), the cut
    reach: vec4<f32>,
    // x: point lights a hit asks for their shadow, at most
    lights: vec4<f32>,
    // the ground map: its center (camera-relative x, z), half width; w: whether it is drawn
    ground: vec4<f32>,
};

// What a hit reads of its instance (`RayRow` in ray_scene/tlas.rs).
struct RayRow {
    // bits 0-7: the class; 8: lit from both sides; 9: `tris` names its triangle records;
    // 10: `tile` names its layer; 16-23: the alpha cutoff, 24-31: the texture's coverage
    flags: u32,
    tris: u32,
    tile: u32,
    tint: u32,
    // mean diffuse colour, roughness
    albedo: vec4<f32>,
    // its own glow (cd/m^2), metallic
    emissive: vec4<f32>,
    // the texture's scale (xy) and offset (zw)
    uv: vec4<f32>,
};

@group(1) @binding(0) var<uniform> ray_scene: RayUniform;
@group(1) @binding(1) var ray_still: acceleration_structure<vertex_return>;
@group(1) @binding(2) var ray_moving: acceleration_structure<vertex_return>;
@group(1) @binding(3) var<storage, read> ray_rows: array<RayRow>;
// (Stand-ins until their steps land: triangle records, the tiles, the ground map.)
@group(1) @binding(4) var<storage, read> ray_tris: array<u32>;
@group(1) @binding(5) var ray_tiles: texture_2d_array<f32>;
@group(1) @binding(6) var ray_ground: texture_2d<f32>;

// An instance's class (`RayClass`): a ray names the classes it sees in its mask.
const RAY_SOLID: u32 = 1u;
const RAY_GROUND: u32 = 2u;
const RAY_SWAY: u32 = 4u;
const RAY_LEAF: u32 = 8u;
const RAY_WATER: u32 = 16u;
const RAY_NOCAST: u32 = 32u;
const RAY_SMALL: u32 = 64u;
const RAY_SHADOW_ONLY: u32 = 128u;
// What probe light and world reflections see: what holds its shape and is 0.3 m or more.
const RAY_SEES_SHAPES: u32 = 51u; // SOLID | GROUND | WATER | NOCAST
// What casts a shadow of those: every face of it stops light on its way from the sun or a
// lamp (`ray_stopped`, which also asks the stand-ins that cast for things that cast none).
const RAY_CASTS: u32 = 3u; // SOLID | GROUND

const RAY_ROW_DOUBLE_SIDED: u32 = 256u;

// No row: the ray hit nothing.
const RAY_NONE: u32 = 0xffffffffu;
// The smallest normal f32: where a query starts, so that a hit at exactly 0 doesn't count.
const RAY_T_MIN: f32 = 1.17549435e-38;

struct RayHit {
    // distance along the ray; the ray's end when it hit nothing
    t: f32,
    // the instance's row (`RAY_NONE`: nothing)
    row: u32,
    // the hit triangle's normal in the scene's space, unnormalized, toward the side its
    // corners run counter-clockwise from (zero for a ray that stops at any hit)
    n: vec3<f32>,
};

// The nearest hit of a camera-relative ray within `t_max` among the instances of `classes`,
// every triangle taken whole (none is asked, whatever its geometry says): no face is culled,
// and a hit counts at 0 < t < t_max (a query takes tmin <= t <= tmax, so it starts at the
// smallest normal float and ends one float short). `any_hit`: stop at the first hit found (a
// shadow ray); its `n` is zero. The moving structure is asked only for what is nearer than
// the still one's hit; a structure with no instance is not asked.
fn ray_whole(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, classes: u32, any_hit: bool) -> RayHit {
    var best: RayHit;
    best.t = t_max;
    best.row = RAY_NONE;
    best.n = vec3<f32>(0.0);
    // (Each query is a variable of the function itself: naga takes no query declared in a
    // loop's body.)
    var still: ray_query<vertex_return>;
    var moving: ray_query<vertex_return>;
    // Nothing to cross (a query's range must be a number, at least 0).
    if (!(t_max > 0.0)) { return best; }
    let o = origin - ray_scene.anchor.xyz;
    var flags = RAY_FLAG_FORCE_OPAQUE;
    if (any_hit) { flags = RAY_FLAG_FORCE_OPAQUE | RAY_FLAG_TERMINATE_ON_FIRST_HIT; }
    if (ray_scene.counts.x > 0u) {
        let t_end = bitcast<f32>(bitcast<u32>(best.t) - 1u);
        rayQueryInitialize(&still, ray_still, RayDesc(flags, classes, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&still)) {}
        let hit = rayQueryGetCommittedIntersection(&still);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
            best.t = hit.t;
            best.row = hit.instance_custom_data;
            if (any_hit) { return best; }
            // The corners in the mesh's space, in index order; a normal goes to the scene
            // through the transposed inverse: the columns of scene -> mesh.
            let v = getCommittedHitVertexPositions(&still);
            let n = cross(v[1] - v[0], v[2] - v[0]);
            let m = hit.world_to_object;
            best.n = vec3<f32>(dot(m[0], n), dot(m[1], n), dot(m[2], n));
        }
    }
    if (ray_scene.counts.y > 0u) {
        let t_end = bitcast<f32>(bitcast<u32>(best.t) - 1u);
        rayQueryInitialize(&moving, ray_moving, RayDesc(flags, classes, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&moving)) {}
        let hit = rayQueryGetCommittedIntersection(&moving);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
            best.t = hit.t;
            best.row = hit.instance_custom_data;
            if (any_hit) { return best; }
            let v = getCommittedHitVertexPositions(&moving);
            let n = cross(v[1] - v[0], v[2] - v[0]);
            let m = hit.world_to_object;
            best.n = vec3<f32>(dot(m[0], n), dot(m[1], n), dot(m[2], n));
        }
    }
    return best;
}

// What stops light on its way along a camera-relative ray within `t_max` (a shadow ray, toward
// the sun or a lamp): its row, `RAY_NONE` when the light arrives. Light is stopped by any face
// of what casts a shadow, and by a stand-in that casts for a thing that casts none itself
// (`RAY_SHADOW_ONLY`) where the ray goes into it: at a face whose corners run counter-clockwise
// as the ray sees them, the outside of a closed shape. A stand-in is such a shape around the
// thing it casts for or within it, and the thing's own surface may lie inside it (a far wall's
// core 8 cm inside the block that casts for the wall): light that leaves a stand-in is not
// stopped by it, else such a thing were shaded by its own stand-in at every hit. (The shadow
// maps hide that with their bias; a ray has none.) What a shadow ray meets is what the shadow
// maps draw. A structure with no stand-in is not asked for them.
fn ray_stopped(origin: vec3<f32>, dir: vec3<f32>, t_max: f32) -> u32 {
    var still: ray_query<vertex_return>;
    var moving: ray_query<vertex_return>;
    let caster = ray_whole(origin, dir, t_max, RAY_CASTS, true).row;
    if (caster != RAY_NONE || !(t_max > 0.0)) { return caster; }
    let o = origin - ray_scene.anchor.xyz;
    let flags = RAY_FLAG_FORCE_OPAQUE | RAY_FLAG_TERMINATE_ON_FIRST_HIT | RAY_FLAG_CULL_BACK_FACING;
    let t_end = bitcast<f32>(bitcast<u32>(t_max) - 1u);
    if (ray_scene.stand_ins.x > 0u) {
        rayQueryInitialize(&still, ray_still, RayDesc(flags, RAY_SHADOW_ONLY, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&still)) {}
        let hit = rayQueryGetCommittedIntersection(&still);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) { return hit.instance_custom_data; }
    }
    if (ray_scene.stand_ins.y > 0u) {
        rayQueryInitialize(&moving, ray_moving, RayDesc(flags, RAY_SHADOW_ONLY, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&moving)) {}
        let hit = rayQueryGetCommittedIntersection(&moving);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) { return hit.instance_custom_data; }
    }
    return RAY_NONE;
}
