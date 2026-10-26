// Impostors (impostor.rs): a mesh baked from IMPOSTOR_GRID² directions into one layer of an
// atlas array, then drawn far away as a single camera-facing quad. Shared by `impostor.wgsl`
// (bake, scene, prepass) and `impostor_shadow.wgsl`. Requires lib_types.
//
// Directions are laid out octahedrally over the whole sphere (`oct_encode`): tile (i, j) holds
// the view from `impostor_tile_dir`, orthographic over the mesh's bounding sphere. Its albedo
// layer holds color and coverage, its normal layer the object-space normal and the depth along
// the view (0 = the sphere's near side, 1 = its far side).

struct ImpostorInfo {
    center_radius: vec4<f32>,  // bounding sphere in mesh space: center xyz, radius
};

const IMPOSTOR_GRID: u32 = 12u;

// The direction (mesh space, unit, from the center toward the viewer) tile `tile` was baked from.
fn impostor_tile_dir(tile: vec2<u32>) -> vec3<f32> {
    let uv = (vec2<f32>(tile) + 0.5) / f32(IMPOSTOR_GRID);
    return oct_decode(uv * 2.0 - 1.0);
}

// Right and up of the view along `d` (columns 0 and 1; column 2 is `d`).
fn impostor_basis(d: vec3<f32>) -> mat3x3<f32> {
    let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(d.y) > 0.99);
    let right = normalize(cross(helper, d));
    let up = cross(d, right);
    return mat3x3<f32>(right, up, d);
}

// The tile nearest to direction `d`, or, with noise in [0, 1)², one of the four around it with
// the bilinear weights (temporal anti-aliasing blends between them).
fn impostor_pick_tile(d: vec3<f32>, noise: vec2<f32>) -> vec2<u32> {
    let f = (oct_encode(d) * 0.5 + 0.5) * f32(IMPOSTOR_GRID) - 0.5;
    let base = floor(f);
    let step = select(vec2<f32>(0.0), vec2<f32>(1.0), noise < f - base);
    let last = f32(IMPOSTOR_GRID - 1u);
    return vec2<u32>(clamp(base + step, vec2<f32>(0.0), vec2<f32>(last)));
}

// Where the view ray through `p` (mesh space, relative to the center; looking along `-toward`)
// crosses the height `height` above tile `tile`'s plane, seen from the tile: its uv within the
// atlas layer, and the tile's basis. Height 0 follows the ray to the tile's own plane, so
// neighboring tiles line up; the surface's baked height then refines it (parallax).
struct ImpostorTexel {
    uv: vec2<f32>,
    inside: bool,
    basis: mat3x3<f32>,
};

fn impostor_texel(tile: vec2<u32>, p: vec3<f32>, toward: vec3<f32>, radius: f32, height: f32) -> ImpostorTexel {
    let basis = impostor_basis(impostor_tile_dir(tile));
    let d = basis[2];
    let on_plane = p - toward * ((dot(p, d) - height) / max(dot(toward, d), 0.2));
    let xy = vec2<f32>(dot(on_plane, basis[0]), dot(on_plane, basis[1])) / radius;
    let local = vec2<f32>(0.5 + 0.5 * xy.x, 0.5 - 0.5 * xy.y);
    var out: ImpostorTexel;
    out.inside = all(local >= vec2<f32>(0.0)) && all(local <= vec2<f32>(1.0));
    out.uv = (vec2<f32>(tile) + clamp(local, vec2<f32>(0.0), vec2<f32>(1.0))) / f32(IMPOSTOR_GRID);
    out.basis = basis;
    return out;
}

// The height of the baked surface above a tile's plane, mesh units, from its stored depth.
fn impostor_height(depth: f32, radius: f32) -> f32 {
    return (0.5 - depth) * 2.0 * radius;
}
