// The ray scene (ray_scene/mod.rs): what the frame's rays trace on ray-tracing hardware. Two
// acceleration structures, one of the instances that hold still and one of those that move,
// a row for each instance that a hit reads, and a record for each triangle built from the mesh
// pool (its uv, its corners' normals, its vertex colour). Composed before the shader that
// traces, after `const RAY_GROUP` (`ray_scene::shader_lib`) and with lib_frame (a hit's tile is
// read through the frame's `linear_repeat`); the scene is group `RAY_GROUP` of its pipeline,
// 1 in the frame's one ray pass.
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
    // shadow stand-ins in the still structure, in the moving one; z: 0; w: the measuring
    // rungs (`RAY_SCENE_LEAVES_SOLID`)
    stand_ins: vec4<u32>,
    // the sun's trace (ray_sun.wgsl; filled whether or not it runs): the reach of the pixels
    // that get a ray, the rays' length (meters), the distance from which they meet swaying
    // plants (meters; 0 at every distance, the largest float never), the tangent of the sun's
    // disc's radius (the measuring rays have a length of their own)
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

@group(RAY_GROUP) @binding(0) var<uniform> ray_scene: RayUniform;
@group(RAY_GROUP) @binding(1) var ray_still: acceleration_structure<vertex_return>;
@group(RAY_GROUP) @binding(2) var ray_moving: acceleration_structure<vertex_return>;
@group(RAY_GROUP) @binding(3) var<storage, read> ray_rows: array<RayRow>;
// The triangle records (ray_scene/tris.rs; ray_attr.wgsl writes them and says what a record
// holds).
@group(RAY_GROUP) @binding(4) var<storage, read> ray_tris: array<u32>;
// The tiles (ray_scene/tiles.rs, filled by ray_tiles.wgsl): a material's base colour in layer
// `RayRow.tile` at 128 texels, every level down to one texel, its mean; a 1x1 stand-in until
// the scene's first fill. (The ground map is a stand-in until step 11 draws it.)
@group(RAY_GROUP) @binding(5) var ray_tiles: texture_2d_array<f32>;
@group(RAY_GROUP) @binding(6) var ray_ground: texture_2d<f32>;

// An instance's class (`RayClass`): a ray names the classes it sees in its mask.
const RAY_SOLID: u32 = 1u;
const RAY_GROUND: u32 = 2u;
const RAY_SWAY: u32 = 4u;
const RAY_LEAF: u32 = 8u;
const RAY_WATER: u32 = 16u;
const RAY_NOCAST: u32 = 32u;
const RAY_SMALL: u32 = 64u;
const RAY_SHADOW_ONLY: u32 = 128u;
// What probe light sees: what holds its shape and is 0.3 m or more, every triangle whole (a
// cut-out among them, such as a fence, as a whole card), trunks and branches that sway among
// them (`RAY_SWAY` where they cast, `RAY_NOCAST` where a stand-in casts for them: a tree is
// seen at every level of detail). The leaves are not in it: they reach the probes through the
// canopy map while it holds crowns, and as `RAY_LEAF` cards, whole, while it holds none
// (probe_gi_rays.wgsl).
const RAY_SEES_SHAPES: u32 = 55u; // SOLID | GROUND | SWAY | WATER | NOCAST
// What casts a shadow of those: every face of it stops light on its way from the sun or a
// lamp to a hit, a cut-out's only where its texture is opaque (`ray_stopped`, which also asks
// the stand-ins that cast for things that cast none; the rules are `SHADOW_RULES` in
// probe_gi_rays.wgsl).
const RAY_CASTS: u32 = 7u; // SOLID | GROUND | SWAY
// What the sun's measuring rays see (`trace_sun` in probe_gi_rays.wgsl): what casts at any
// size, and the leaves (a crown's stand-in, swaying cards that cast) through the holes of
// their texture, which prices the leaves' candidates in the sun's trace. Not the `Sway` of
// trunks and branches: within the cascades the shadow maps draw them moving.
const RAY_SUN_CASTS: u32 = 75u; // SOLID | GROUND | LEAF | SMALL
// Every class a view draws: all but `RAY_SHADOW_ONLY` (world reflections).
const RAY_DRAWN: u32 = 127u;

const RAY_ROW_DOUBLE_SIDED: u32 = 256u;
// `RayRow.tris` names the row's triangle records.
const RAY_ROW_HAS_TRIS: u32 = 512u;
// `RayRow.tile` names its layer of `ray_tiles` (`ROW_HAS_TILE`, ray_scene/tlas.rs).
const RAY_ROW_HAS_TILE: u32 = 1024u;
// Levels added to the level a ray's cone picks in a tile (`ray_tile_level`): 0 reads the cone
// as it is.
const RAY_TILE_BIAS: f32 = 0.0;

// No row: the ray hit nothing.
const RAY_NONE: u32 = 0xffffffffu;
// The smallest normal f32: where a query starts, so that a hit at exactly 0 doesn't count.
const RAY_T_MIN: f32 = 1.17549435e-38;

// The rules a ray asks the triangles that are asked by (`ray_first`, `ray_nearest`): those of
// the `Ground` and of the cards cut out by their texture (every `Leaf`, and a cut-out of
// another class such as a fence), the only geometries built to be asked; every other triangle
// stops a ray at any hit, as does one a rule leaves alone.
// The ground stops a ray only at its front (its corners counter-clockwise as the ray sees
// them): a ray from below passes up through it.
const RULE_GROUND_FRONT: u32 = 1u;
// A cut-out card stops a ray where its texture is opaque (the tile's alpha at least the row's
// cutoff), read for the first `(rules >> RULE_TESTS_SHIFT) & 255` of the ray's candidates, by
// chance after them (`RULE_LEAF_CHANCE`).
const RULE_LEAF_TEXTURE: u32 = 2u;
// A cut-out card stops a ray by chance: as often as its texture covers the card (the row's
// coverage), the same draw for the same candidate within a frame.
const RULE_LEAF_CHANCE: u32 = 4u;
// `ray_first`, `ray_stopped`: only the still structure is asked.
const RULE_STILL_ONLY: u32 = 8u;
// A cut-out card stops a ray wherever the ray meets it, as if its texture covered it: what
// `ray_rules` asks in place of the two leaf rules under the measuring rung `ray_leaves_solid`.
const RULE_LEAF_COVERED: u32 = 16u;
// A cut-out card drawn from one side stops a ray only at that side, as the view's raster culls
// its back: what a world reflection asks. A shadow ray leaves it out and meets both faces, as
// the shadow maps draw a cut-out.
const RULE_DRAWN_SIDE: u32 = 32u;
// The rules that ask a cut-out card's candidates.
const RULE_CUT_OUTS: u32 = 22u; // LEAF_TEXTURE | LEAF_CHANCE | LEAF_COVERED
// The rules that ask candidates: without one, every triangle is taken whole (`ray_flags`).
const RULE_ASKS: u32 = 23u;
const RULE_TESTS_SHIFT: u32 = 8u;
// `RayUniform.stand_ins.w`: the measuring rung `ray_leaves_solid` (`shade_skip`): a cut-out
// card's candidate is kept at once (at a side the card is drawn from, for a ray that asks
// `RULE_DRAWN_SIDE`), as if its texture covered the card (what the row prices: the texture
// reads, and the triangles behind the holes).
const RAY_SCENE_LEAVES_SOLID: u32 = 1u;

// The distance along the ray of the hit `ray_first` or `ray_stopped` last found (the ray's
// `t_max` where they found none): the sun's trace keeps a blocker's distance for the penumbra
// it casts (ray_sun.wgsl).
var<private> ray_found_t: f32;

// lowbias32 (Wellons): a well mixed hash of a word.
fn ray_hash(v: u32) -> u32 {
    var h = v;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

// A draw in [0, 1) for candidate `c`: the same for the same candidate within a frame (one
// handed over twice is decided the same way), another the next frame.
fn ray_chance(c: RayIntersection) -> f32 {
    var h = ray_hash(c.instance_custom_data ^ ray_scene.counts.z);
    h = ray_hash(h ^ c.primitive_index);
    h = ray_hash(h ^ bitcast<u32>(c.barycentrics.x));
    h = ray_hash(h ^ bitcast<u32>(c.barycentrics.y));
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

// A draw in [0, 1) for pixel `pixel` this frame, the `k`th of the pixel's: the first is
// interleaved gradient noise moved along by the frame (pixels side by side differ, so a filter
// over a few of them averages it, and each frame differs for 64 frames), the others a hash of
// the pixel, the frame and `k` (they do not follow the first). The first runs across the
// pixels' rows and columns swapped: the noise of `frame_noise` moved along by any amount is
// the same pattern shifted by a constant, so a choice made by `frame_noise` (the cascades'
// dither) would take the draws of one arc alone.
fn ray_noise(pixel: vec2<u32>, k: u32) -> f32 {
    let frame_number = ray_scene.counts.z;
    if (k == 0u) {
        return interleaved_gradient_noise(vec2<f32>(pixel.yx) + 5.588238 * f32(frame_number % 64u));
    }
    let h = ray_hash(ray_hash(ray_hash(pixel.x ^ (k * 0x9e3779b9u)) ^ pixel.y) ^ frame_number);
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

// Whether candidate `c` stops a ray that asks by `rules` (`RULE_*`, as `ray_rules` gives them).
// `tests`: the texture reads the ray made so far, shared by its two structures. A cut-out card
// with no triangle records or no tile (a copy's geometry, a texture not in the tiles yet) is
// taken by chance.
fn ray_accepts(c: RayIntersection, rules: u32, tests: ptr<function, u32>) -> bool {
    let i = c.instance_custom_data;
    // (The row's fields one by one: the rest of it only where its texture is read.)
    let flags = ray_rows[i].flags;
    if ((flags & 255u) == RAY_GROUND) { return (rules & RULE_GROUND_FRONT) == 0u || c.front_face; }
    // Else a card cut out by its texture: the only other geometry built to be asked.
    if ((rules & RULE_CUT_OUTS) == 0u) { return true; }
    if ((rules & RULE_DRAWN_SIDE) != 0u && !c.front_face && (flags & RAY_ROW_DOUBLE_SIDED) == 0u) {
        return false;
    }
    if ((rules & RULE_LEAF_COVERED) != 0u) { return true; }
    let needs = RAY_ROW_HAS_TRIS | RAY_ROW_HAS_TILE;
    if ((rules & RULE_LEAF_TEXTURE) != 0u && (flags & needs) == needs
        && *tests < ((rules >> RULE_TESTS_SHIFT) & 255u)) {
        *tests += 1u;
        let tris = ray_rows[i].tris;
        let at = ray_tri_at(tris, c.primitive_index);
        let uv = ray_tri_uv(tris, at, c.barycentrics) * ray_rows[i].uv.xy + ray_rows[i].uv.zw;
        // Level 0: a coarser one averages alpha, and would move the card's cut edge.
        let alpha = textureSampleLevel(ray_tiles, linear_repeat, uv, ray_rows[i].tile, 0.0).a;
        return alpha >= f32((flags >> 16u) & 255u) / 255.0;
    }
    return ray_chance(c) < f32(flags >> 24u) / 255.0;
}

// The flags a ray that asks by `rules` is traced with: every triangle whole when it asks none.
fn ray_flags(rules: u32) -> u32 {
    return select(0u, RAY_FLAG_FORCE_OPAQUE, (rules & RULE_ASKS) == 0u);
}

// The rules a ray's candidates are asked by this frame: with the measuring rung
// `ray_leaves_solid`, a ray that asks cut-out cards keeps each candidate on a side the card is
// drawn from at once (`RULE_LEAF_COVERED`; the candidates still come: the ray's flags follow
// the rules it was given).
fn ray_rules(rules: u32) -> u32 {
    if ((ray_scene.stand_ins.w & RAY_SCENE_LEAVES_SOLID) != 0u && (rules & RULE_CUT_OUTS) != 0u) {
        return (rules & ~(RULE_LEAF_TEXTURE | RULE_LEAF_CHANCE)) | RULE_LEAF_COVERED;
    }
    return rules;
}

struct RayHit {
    // distance along the ray; the ray's end when it hit nothing
    t: f32,
    // the instance's row (`RAY_NONE`: nothing)
    row: u32,
    // the hit triangle's normal in the scene's space, unnormalized, toward the side its
    // corners run counter-clockwise from (zero for a ray that stops at any hit)
    n: vec3<f32>,
    // the triangle within its geometry (`primitive_index`; its record is `RayRow.tris` plus
    // this), `RAY_NONE` where the hit carries no more than the fields above (nothing hit, a
    // ray that stops at any hit)
    primitive: u32,
    // where on the triangle: the weights of corners 1 and 2 (corner 0 weighs 1 - x - y)
    bary: vec2<f32>,
    // whether the ray met the side whose corners run counter-clockwise as it sees them, told
    // in the mesh's space (a mirrored instance keeps it). Shading tells the side by `n`
    // instead (`ray_surface`).
    front: bool,
    // scene -> mesh without the translation (`world_to_object`'s first three columns): a
    // normal of the mesh goes into the scene through its transpose, and |det| is how much
    // mesh volume a cubic meter of the scene holds
    to_object: mat3x3<f32>,
};

// What a committed hit gives a ray that asks what it meets, past its distance and row: the
// face's normal in the scene (the corners in the mesh's space, in index order, through the
// transposed inverse: the columns of scene -> mesh), the triangle, where on it, the side, and
// the turn back into the mesh.
fn ray_hit_whole(best: ptr<function, RayHit>, hit: RayIntersection, v: array<vec3<f32>, 3>) {
    let m = hit.world_to_object;
    let n = cross(v[1] - v[0], v[2] - v[0]);
    (*best).n = vec3<f32>(dot(m[0], n), dot(m[1], n), dot(m[2], n));
    (*best).primitive = hit.primitive_index;
    (*best).bary = hit.barycentrics;
    (*best).front = hit.front_face;
    (*best).to_object = mat3x3<f32>(m[0], m[1], m[2]);
}

// The nearest hit of a camera-relative ray within `t_max` among the instances of `classes`,
// every triangle taken whole (none is asked, whatever its geometry says): no face is culled,
// and a hit counts at 0 < t < t_max (a query takes tmin <= t <= tmax, so it starts at the
// smallest normal float and ends one float short). The moving structure is asked only for
// what is nearer than the still one's hit; a structure with no instance is not asked. (The
// probes' rays: `ray_nearest` asks the triangles that are asked.)
fn ray_whole(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, classes: u32) -> RayHit {
    var best: RayHit;
    best.t = t_max;
    best.row = RAY_NONE;
    best.n = vec3<f32>(0.0);
    best.primitive = RAY_NONE;
    // (Each query is a variable of the function itself: naga takes no query declared in a
    // loop's body.)
    var still: ray_query<vertex_return>;
    var moving: ray_query<vertex_return>;
    // Nothing to cross (a query's range must be a number, at least 0).
    if (!(t_max > 0.0)) { return best; }
    let o = origin - ray_scene.anchor.xyz;
    let flags = RAY_FLAG_FORCE_OPAQUE;
    if (ray_scene.counts.x > 0u) {
        let t_end = bitcast<f32>(bitcast<u32>(best.t) - 1u);
        rayQueryInitialize(&still, ray_still, RayDesc(flags, classes, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&still)) {}
        let hit = rayQueryGetCommittedIntersection(&still);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
            best.t = hit.t;
            best.row = hit.instance_custom_data;
            ray_hit_whole(&best, hit, getCommittedHitVertexPositions(&still));
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
            ray_hit_whole(&best, hit, getCommittedHitVertexPositions(&moving));
        }
    }
    return best;
}

// What stops light on its way along a camera-relative ray within `t_max` (a shadow ray, toward
// the sun or a lamp): its row, `RAY_NONE` when the light arrives. Light is stopped by the
// instances of `classes` (what casts a shadow: `RAY_CASTS`, `RAY_SUN_CASTS`), their triangles
// that are asked asked by `rules` (`ray_first`; 0: any face stops it), and by a stand-in that
// casts for a thing that casts none itself (`RAY_SHADOW_ONLY`) where the ray goes into it: at a
// face whose corners run counter-clockwise as the ray sees them, the outside of a closed
// shape. A stand-in is such a shape around the thing it casts for or within it, and the
// thing's own surface may lie inside it (a far wall's core 8 cm inside the block that casts
// for the wall): light that leaves a stand-in is not stopped by it, else such a thing were
// shaded by its own stand-in at every hit. (The shadow maps hide that with their bias; a ray
// has none.) What a shadow ray meets is what the shadow maps draw, but for the classes the
// caller leaves out (`classes`: a hit's shadow rays pass what is under 0.3 m, and take the
// canopy map's leaves in place of the cards at a probe's hit). A structure with no stand-in is
// not asked for them, nor the moving one with `RULE_STILL_ONLY`.
fn ray_stopped(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, classes: u32, rules: u32) -> u32 {
    var still: ray_query<vertex_return>;
    var moving: ray_query<vertex_return>;
    let caster = ray_first(origin, dir, t_max, classes, rules);
    if (caster != RAY_NONE || !(t_max > 0.0)) { return caster; }
    let o = origin - ray_scene.anchor.xyz;
    let flags = RAY_FLAG_FORCE_OPAQUE | RAY_FLAG_TERMINATE_ON_FIRST_HIT | RAY_FLAG_CULL_BACK_FACING;
    let t_end = bitcast<f32>(bitcast<u32>(t_max) - 1u);
    if (ray_scene.stand_ins.x > 0u) {
        rayQueryInitialize(&still, ray_still, RayDesc(flags, RAY_SHADOW_ONLY, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&still)) {}
        let hit = rayQueryGetCommittedIntersection(&still);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
            ray_found_t = hit.t;
            return hit.instance_custom_data;
        }
    }
    if (ray_scene.stand_ins.y > 0u && (rules & RULE_STILL_ONLY) == 0u) {
        rayQueryInitialize(&moving, ray_moving, RayDesc(flags, RAY_SHADOW_ONLY, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&moving)) {}
        let hit = rayQueryGetCommittedIntersection(&moving);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
            ray_found_t = hit.t;
            return hit.instance_custom_data;
        }
    }
    return RAY_NONE;
}

// The row of what stops a camera-relative ray within `t_max` among the instances of
// `classes`, the triangles that are asked asked by `rules` (`RULE_*`; 0: every triangle
// whole): the first hit found, not the nearest (`RAY_NONE`: nothing). Both structures, the
// still one alone with `RULE_STILL_ONLY`; one with no instance is not asked. (As in
// `ray_whole`, the queries are the function's own variables, and a loop holds nothing but the
// ask: no `break`, no `return`.)
fn ray_first(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, classes: u32, rules: u32) -> u32 {
    var still: ray_query<vertex_return>;
    var moving: ray_query<vertex_return>;
    var tests = 0u;
    ray_found_t = t_max;
    if (!(t_max > 0.0)) { return RAY_NONE; }
    let o = origin - ray_scene.anchor.xyz;
    let flags = ray_flags(rules) | RAY_FLAG_TERMINATE_ON_FIRST_HIT;
    let asks = ray_rules(rules);
    let t_end = bitcast<f32>(bitcast<u32>(t_max) - 1u);
    if (ray_scene.counts.x > 0u) {
        rayQueryInitialize(&still, ray_still, RayDesc(flags, classes, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&still)) {
            if (ray_accepts(rayQueryGetCandidateIntersection(&still), asks, &tests)) {
                rayQueryConfirmIntersection(&still);
            }
        }
        let hit = rayQueryGetCommittedIntersection(&still);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
            ray_found_t = hit.t;
            return hit.instance_custom_data;
        }
    }
    if (ray_scene.counts.y > 0u && (rules & RULE_STILL_ONLY) == 0u) {
        rayQueryInitialize(&moving, ray_moving, RayDesc(flags, classes, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&moving)) {
            if (ray_accepts(rayQueryGetCandidateIntersection(&moving), asks, &tests)) {
                rayQueryConfirmIntersection(&moving);
            }
        }
        let hit = rayQueryGetCommittedIntersection(&moving);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
            ray_found_t = hit.t;
            return hit.instance_custom_data;
        }
    }
    return RAY_NONE;
}

// Whether anything among `classes` stops a camera-relative ray within `t_max` (`ray_first`).
fn ray_any(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, classes: u32, rules: u32) -> bool {
    return ray_first(origin, dir, t_max, classes, rules) != RAY_NONE;
}

// The nearest hit of a camera-relative ray within `t_max` among the instances of `classes`, as
// `ray_whole` gives it, the triangles that are asked asked by `rules` (`RULE_*`): the still
// structure first, the moving one only for what is nearer.
fn ray_nearest(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, classes: u32, rules: u32) -> RayHit {
    var best: RayHit;
    best.t = t_max;
    best.row = RAY_NONE;
    best.n = vec3<f32>(0.0);
    best.primitive = RAY_NONE;
    var still: ray_query<vertex_return>;
    var moving: ray_query<vertex_return>;
    var tests = 0u;
    if (!(t_max > 0.0)) { return best; }
    let o = origin - ray_scene.anchor.xyz;
    let flags = ray_flags(rules);
    let asks = ray_rules(rules);
    if (ray_scene.counts.x > 0u) {
        let t_end = bitcast<f32>(bitcast<u32>(best.t) - 1u);
        rayQueryInitialize(&still, ray_still, RayDesc(flags, classes, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&still)) {
            if (ray_accepts(rayQueryGetCandidateIntersection(&still), asks, &tests)) {
                rayQueryConfirmIntersection(&still);
            }
        }
        let hit = rayQueryGetCommittedIntersection(&still);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
            best.t = hit.t;
            best.row = hit.instance_custom_data;
            ray_hit_whole(&best, hit, getCommittedHitVertexPositions(&still));
        }
    }
    if (ray_scene.counts.y > 0u) {
        let t_end = bitcast<f32>(bitcast<u32>(best.t) - 1u);
        rayQueryInitialize(&moving, ray_moving, RayDesc(flags, classes, RAY_T_MIN, t_end, o, dir));
        while (rayQueryProceed(&moving)) {
            if (ray_accepts(rayQueryGetCandidateIntersection(&moving), asks, &tests)) {
                rayQueryConfirmIntersection(&moving);
            }
        }
        let hit = rayQueryGetCommittedIntersection(&moving);
        if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
            best.t = hit.t;
            best.row = hit.instance_custom_data;
            ray_hit_whole(&best, hit, getCommittedHitVertexPositions(&moving));
        }
    }
    return best;
}

// The first word of the record of triangle `primitive` (a hit's `primitive_index`) of a row
// whose first record is `tris` (5 words a record). Only for a row with `RAY_ROW_HAS_TRIS`.
fn ray_tri_at(tris: u32, primitive: u32) -> u32 {
    return (tris + primitive) * 5u;
}

// The smooth normal at a hit, in the mesh's space (unit). `bary`: the hit's barycentrics, the
// weights of corners 1 and 2.
fn ray_tri_normal(at: u32, bary: vec2<f32>) -> vec3<f32> {
    let a = unpack4x8snorm(ray_tris[at + 3u]);
    let b = unpack4x8snorm(ray_tris[at + 4u]);
    let n0 = oct_decode(a.xy);
    let n = n0 * (1.0 - bary.x - bary.y) + oct_decode(a.zw) * bary.x + oct_decode(b.xy) * bary.y;
    let d = dot(n, n);
    if (d > 1e-8) { return n * inverseSqrt(d); }
    return n0;
}

// The mesh's texture coordinate at a hit (the row's scale and offset apply after). `tris`:
// the row's first record; the geometry's header is the record before it.
fn ray_tri_uv(tris: u32, at: u32, bary: vec2<f32>) -> vec2<f32> {
    let h = (tris - 1u) * 5u;
    let lo = vec2<f32>(bitcast<f32>(ray_tris[h]), bitcast<f32>(ray_tris[h + 1u]));
    let size = vec2<f32>(bitcast<f32>(ray_tris[h + 2u]), bitcast<f32>(ray_tris[h + 3u]));
    let reach = bitcast<f32>(ray_tris[h + 4u]);
    let edges = unpack2x16snorm(ray_tris[at + 1u]) * bary.x + unpack2x16snorm(ray_tris[at + 2u]) * bary.y;
    return lo + unpack2x16unorm(ray_tris[at]) * size + edges * reach;
}

// The triangle's mean vertex colour, linear: it multiplies the albedo.
fn ray_tri_color(at: u32) -> vec3<f32> {
    let w = ray_tris[at + 4u] >> 16u;
    let c = vec3<f32>(f32(w >> 11u) / 31.0, f32((w >> 5u) & 63u) / 63.0, f32(w & 31u) / 31.0);
    return c * c;
}

// A hit as a surface (`ray_surface`): what lights it (`hit_radiance` in probe_gi.wgsl).
struct RaySurface {
    // the face's own normal (unit), on the ray's side
    face: vec3<f32>,
    // the normal it is shaded with (unit), on the ray's side of the face: its corners' normals
    // where the row has triangle records, else exactly `face`
    n: vec3<f32>,
    // diffuse albedo (linear): the row's mean colour, the tile's pattern around that mean and
    // the triangle's vertex colour
    albedo: vec3<f32>,
    // its own glow (cd/m^2)
    emissive: vec3<f32>,
    roughness: f32,
    metallic: f32,
    // its class (`RAY_SOLID` .. `RAY_SHADOW_ONLY`, the row's bits 0-7)
    class_bit: u32,
    // the ray met the back of a single-sided face: it started inside something
    inside: bool,
};

// Hit `hit` of a ray along `dir` (unit) as a surface. `cone`: the ray's width where it started
// (meters) and how fast it widens (meters per meter), which pick the tile's level
// (`ray_tile_level`). The side the ray met is told by the face's normal, as every scene of the
// probes tells it; the back of a single-sided face is `inside`. A row without records is its
// face and its row exactly as before the records. The ground keeps its row's colour until
// step 11 reads `ray_ground` here.
fn ray_surface(hit: RayHit, dir: vec3<f32>, cone: vec2<f32>) -> RaySurface {
    let row = ray_rows[hit.row];
    var s: RaySurface;
    var face = normalize(hit.n);
    let back = dot(face, dir) > 0.0;
    if (back) { face = -face; }
    s.face = face;
    s.n = face;
    s.albedo = row.albedo.rgb;
    s.emissive = row.emissive.rgb;
    s.roughness = row.albedo.w;
    s.metallic = row.emissive.w;
    s.class_bit = row.flags & 255u;
    s.inside = back && (row.flags & RAY_ROW_DOUBLE_SIDED) == 0u;
    if ((row.flags & RAY_ROW_HAS_TRIS) == 0u || hit.primitive == RAY_NONE) { return s; }
    let at = ray_tri_at(row.tris, hit.primitive);
    // The corners' normals, into the scene through the transposed inverse (the columns of
    // scene -> mesh), turned with the face; one that leans into the face is mirrored out of it,
    // as mesh.wgsl does for a thin sheet, so `n` is never below the face's plane.
    let m = ray_tri_normal(at, hit.bary);
    var n = normalize(vec3<f32>(dot(hit.to_object[0], m), dot(hit.to_object[1], m), dot(hit.to_object[2], m)));
    if (back) { n = -n; }
    let into = dot(n, face);
    if (into < 0.0) { n = normalize(n - 2.0 * into * face); }
    s.n = n;
    var albedo = row.albedo.rgb * ray_tri_color(at);
    if ((row.flags & RAY_ROW_HAS_TILE) != 0u) {
        // The tile's pattern around the tile's own mean (its last level): `RayRow.albedo`
        // holds the texture's mean already (look.rs `material_light`), so the texture counts
        // once and the hit keeps the row's brightness on average.
        let uv = ray_tri_uv(row.tris, at, hit.bary) * row.uv.xy + row.uv.zw;
        let level = ray_tile_level(row, at, hit, dir, cone);
        let texel = textureSampleLevel(ray_tiles, linear_repeat, uv, row.tile, level).rgb;
        let mean = textureLoad(ray_tiles, vec2<u32>(0u), row.tile, textureNumLevels(ray_tiles) - 1u).rgb;
        albedo *= texel / max(mean, vec3<f32>(1.0 / 255.0));
    }
    s.albedo = min(albedo, vec3<f32>(0.95));
    return s;
}

// The level of `ray_tiles` that hit `hit` reads (ray cones, Akenine-Moller et al. 2019): the
// cone's width at the hit over the cosine of the face to the ray, in the triangle's own texels.
// Twice the triangle's area is |n| / |det to_object| in the scene and
// size^2 |S.x S.y| |e1 x e2| in level-0 texels (S the row's uv scale, e1 and e2 its record's uv
// edges), so (footprint in texels)^2 = W^2 size^2 |S.x S.y| |e1 x e2| |det| |n| / (n.d)^2.
fn ray_tile_level(row: RayRow, at: u32, hit: RayHit, dir: vec3<f32>, cone: vec2<f32>) -> f32 {
    let last = f32(textureNumLevels(ray_tiles) - 1u);
    let reach = bitcast<f32>(ray_tris[(row.tris - 1u) * 5u + 4u]);
    let e1 = unpack2x16snorm(ray_tris[at + 1u]) * reach;
    let e2 = unpack2x16snorm(ray_tris[at + 2u]) * reach;
    let size = f32(textureDimensions(ray_tiles).x);
    let texel_area = abs((e1.x * e2.y - e1.y * e2.x) * row.uv.x * row.uv.y) * size * size;
    // A triangle thinner in uv than its record's step (reach / 32767) reads a zero area: its
    // texel density is unknown, so it reads the tile's mean rather than its finest level (a
    // probe ray would otherwise read one texel at any distance).
    if (!(texel_area > 0.0)) { return last; }
    let width = cone.x + cone.y * hit.t;
    let nd = dot(hit.n, dir);
    let footprint2 = width * width * texel_area * abs(determinant(hit.to_object)) * length(hit.n)
        / max(nd * nd, 1e-30);
    return clamp(0.5 * log2(max(footprint2, 1e-30)) + RAY_TILE_BIAS, 0.0, last);
}
