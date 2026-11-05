// Impostors: baking a mesh into its atlas layer, and drawing it as a quad in the scene and the
// depth prepass. Appended to mesh.wgsl (its materials, `evaluate_material` and vertex math) and
// impostor_common.wgsl. Requires lib_types + lib_frame.

@group(3) @binding(0) var impostor_albedo: texture_2d_array<f32>;
@group(3) @binding(1) var impostor_normal: texture_2d_array<f32>;
@group(3) @binding(2) var impostor_sampler: sampler;
@group(3) @binding(3) var<storage, read> impostors: array<ImpostorInfo>;

// ---------------------------------------------------------------- bake
// One draw per tile, each into its own viewport of the layer, instance index = tile. The
// instance's model matrix is the identity (mesh space) and `extra.y` is the layer.

struct BakeOut {
    @location(0) albedo: vec4<f32>,
    @location(1) normal_depth: vec4<f32>,
};

@vertex
fn vs_impostor_bake(in: VertexIn, @builtin(instance_index) tile: u32) -> VertexOut {
    let sphere = impostors[in.extra.y].center_radius;
    let basis = impostor_basis(impostor_tile_dir(vec2<u32>(tile % IMPOSTOR_GRID, tile / IMPOSTOR_GRID)));
    let q = in.position - sphere.xyz;
    var out = standard_vertex(in);
    // Seen from far along the tile's direction: detail fades as it does at a distance.
    out.rel = q - basis[2] * sphere.w * 8.0;
    out.clip = vec4<f32>(
        dot(q, basis[0]) / sphere.w,
        dot(q, basis[1]) / sphere.w,
        0.5 - 0.5 * dot(q, basis[2]) / sphere.w,
        1.0,
    );
    return out;
}

@fragment
fn fs_impostor_bake(in: VertexOut, @builtin(front_facing) front: bool) -> BakeOut {
    let mat = materials[in.material];
    let m = evaluate_material(surface_in(in, front), false);
    if ((mat.flags.x & 15u) == 1u && m.base.a < mat.params.w) {
        discard;
    }
    var out: BakeOut;
    // Occlusion apart (it darkens ambient light only, as on the mesh: baked into the albedo it
    // dimmed sunlit fur and cavities to a dark block).
    out.albedo = vec4<f32>(m.albedo, 1.0);
    // Mesh space (the identity model matrix makes the shaded normal a mesh-space one),
    // octahedral, then the occlusion and the depth.
    out.normal_depth = vec4<f32>(oct_encode(m.normal) * 0.5 + 0.5, m.occlusion, in.clip.z);
    return out;
}

// ---------------------------------------------------------------- scene and prepass

struct ImpostorIn {
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) n0: vec4<f32>,
    @location(8) n1: vec4<f32>,
    @location(9) n2: vec4<f32>,
    @location(10) extra: vec4<u32>,  // material index, layer
    @location(11) p0: vec4<f32>,
    @location(12) p1: vec4<f32>,
    @location(13) p2: vec4<f32>,
};

struct ImpostorOut {
    @builtin(position) @invariant clip: vec4<f32>,
    // Mesh-space point on the quad, relative to the sphere's center.
    @location(0) plane: vec3<f32>,
    // Mesh-space direction toward the camera.
    @location(1) @interpolate(flat) toward: vec3<f32>,
    @location(2) @interpolate(flat) m0: vec4<f32>,
    @location(3) @interpolate(flat) m1: vec4<f32>,
    @location(4) @interpolate(flat) m2: vec4<f32>,
    @location(5) @interpolate(flat) n0: vec3<f32>,
    @location(6) @interpolate(flat) n1: vec3<f32>,
    @location(7) @interpolate(flat) n2: vec3<f32>,
    // How far the center moved since last frame, relative to last frame's camera.
    @location(8) @interpolate(flat) moved: vec3<f32>,
    @location(9) @interpolate(flat) ids: vec2<u32>,
    // Its dither (`n0.w`, `dither_keeps` in lib_frame.wgsl; 0: drawn whole).
    @location(10) @interpolate(flat) fade: f32,
    // The snow a bone plant's species holds (`plant_snow_hold`; 0 for other meshes).
    @location(11) @interpolate(flat) snow: f32,
};

fn impostor_corner(v: u32) -> vec2<f32> {
    let corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    return corners[v];
}

fn rows_apply(m0: vec4<f32>, m1: vec4<f32>, m2: vec4<f32>, p: vec4<f32>) -> vec3<f32> {
    return vec3<f32>(dot(m0, p), dot(m1, p), dot(m2, p));
}

// Mesh space from a camera-relative offset: the inverse of the model's linear part, for
// rotation and (near-)uniform scale.
fn rows_unapply(m0: vec4<f32>, m1: vec4<f32>, m2: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = m0.xyz * v.x + m1.xyz * v.y + m2.xyz * v.z;
    let s2 = dot(vec3<f32>(m0.x, m1.x, m2.x), vec3<f32>(m0.x, m1.x, m2.x));
    return t / max(s2, 1e-12);
}

// An instance's model rows and normal matrix columns.
struct ImpostorPose {
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
    n0: vec3<f32>,
    n1: vec3<f32>,
    n2: vec3<f32>,
};

// A bone plant's impostor (wind table `entry`, lib_wind.wgsl) tilted about its trunk's root so
// its top moves by the trunk's lean (`plant_lean`), this frame's or last frame's (`prev`), as
// the mesh's does: whole hillsides of far trees bow in a storm. The pose's normal columns turn
// with it (the turn is a rotation).
fn impostor_leaned(pose: ImpostorPose, entry: u32, prev: bool) -> ImpostorPose {
    let mesh = plant_mesh(entry);
    let sp = plant_species(mesh.y);
    let header = plant_wind_words[0];
    let root_words = plant_word(header.z + 2u * mesh.x);
    let root = root_words.xyz;
    let height = max(root_words.w, 1e-3);
    var air = PlantAir(frame.plant_wind, frame.gusts, vec2<f32>(pose.m0.w, pose.m2.w) + frame.gust_offset.xy,
        vec2<f32>(0.0), false, 0u, 1.0);
    if (prev) {
        air = PlantAir(frame.plant_wind_prev, frame.gusts_prev, vec2<f32>(pose.m0.w, pose.m2.w) + frame.gust_offset.zw,
            vec2<f32>(0.0), true, 0u, 1.0);
    }
    let dir = plant_wind_dir(pose.n0, pose.n1, pose.n2, air.wind);
    let lean = plant_lean(sp, air, dir, height, plant_hash(air.root_at));
    let amount = length(lean);
    if (amount < 1e-4) { return pose; }
    let k = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(lean.x, 0.0, lean.y)));
    let angle = atan2(amount, height);
    let turn = mat3x3<f32>(
        plant_turn(vec3<f32>(1.0, 0.0, 0.0), k, angle),
        plant_turn(vec3<f32>(0.0, 1.0, 0.0), k, angle),
        plant_turn(vec3<f32>(0.0, 0.0, 1.0), k, angle),
    );
    // Model: M (root + R (p - root)): linear part L R, offset L root + t - L R root.
    let linear = mat3x3<f32>(
        vec3<f32>(pose.m0.x, pose.m1.x, pose.m2.x),
        vec3<f32>(pose.m0.y, pose.m1.y, pose.m2.y),
        vec3<f32>(pose.m0.z, pose.m1.z, pose.m2.z),
    );
    let turned = linear * turn;
    let offset = vec3<f32>(pose.m0.w, pose.m1.w, pose.m2.w) + linear * root - turned * root;
    let normals = mat3x3<f32>(pose.n0, pose.n1, pose.n2) * turn;
    var out: ImpostorPose;
    out.m0 = vec4<f32>(turned[0].x, turned[1].x, turned[2].x, offset.x);
    out.m1 = vec4<f32>(turned[0].y, turned[1].y, turned[2].y, offset.y);
    out.m2 = vec4<f32>(turned[0].z, turned[1].z, turned[2].z, offset.z);
    out.n0 = normals[0];
    out.n1 = normals[1];
    out.n2 = normals[2];
    return out;
}

@vertex
fn vs_impostor(input: ImpostorIn, @builtin(vertex_index) v: u32) -> ImpostorOut {
    var in = input;
    // Bits 0-15 the layer, 16-31 a bone plant's wind table entry.
    let entry = plant_wind_entry(in.extra.y);
    in.extra.y = in.extra.y & 0xffffu;
    if (entry != 0u) {
        let now = impostor_leaned(ImpostorPose(in.m0, in.m1, in.m2, in.n0.xyz, in.n1.xyz, in.n2.xyz), entry, false);
        let last = impostor_leaned(ImpostorPose(in.p0, in.p1, in.p2, in.n0.xyz, in.n1.xyz, in.n2.xyz), entry, true);
        in.m0 = now.m0;
        in.m1 = now.m1;
        in.m2 = now.m2;
        in.n0 = vec4<f32>(now.n0, in.n0.w);
        in.n1 = vec4<f32>(now.n1, in.n1.w);
        in.n2 = vec4<f32>(now.n2, in.n2.w);
        in.p0 = last.m0;
        in.p1 = last.m1;
        in.p2 = last.m2;
    }
    let sphere = impostors[in.extra.y].center_radius;
    let center = rows_apply(in.m0, in.m1, in.m2, vec4<f32>(sphere.xyz, 1.0));
    let scale = length(vec3<f32>(in.m0.x, in.m1.x, in.m2.x));
    let radius = sphere.w * scale;
    let to_camera = normalize(-center);
    let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(to_camera.y) > 0.99);
    let right = normalize(cross(helper, to_camera));
    let up = cross(to_camera, right);
    let c = impostor_corner(v);
    let offset = (right * c.x + up * c.y) * radius;
    var out: ImpostorOut;
    out.clip = frame.view_proj * vec4<f32>(center + offset, 1.0);
    out.plane = rows_unapply(in.m0, in.m1, in.m2, offset);
    out.toward = normalize(rows_unapply(in.m0, in.m1, in.m2, to_camera));
    out.m0 = in.m0;
    out.m1 = in.m1;
    out.m2 = in.m2;
    out.n0 = in.n0.xyz;
    out.n1 = in.n1.xyz;
    out.n2 = in.n2.xyz;
    out.moved = rows_apply(in.p0, in.p1, in.p2, vec4<f32>(sphere.xyz, 1.0)) - center;
    out.ids = in.extra.xy;
    out.fade = in.n0.w;
    out.snow = select(0.0, plant_snow_hold(entry), entry != 0u);
    return out;
}

struct ImpostorSurface {
    albedo: vec3<f32>,
    occlusion: f32,
    normal: vec3<f32>,  // world, unit
    rel: vec3<f32>,     // camera-relative position of the surface
    depth: f32,         // its depth buffer value
};

// The baked surface behind this pixel, or discard where the mesh doesn't cover it. The tile is
// picked stochastically from the four around the view direction (the same in the prepass and
// the scene pass, so their depths match).
fn impostor_surface(in: ImpostorOut) -> ImpostorSurface {
    // Cross-fading in, the pixels the mesh leaves (`fs_prepass` in mesh.wgsl); or dithered out
    // with its mesh (`DitherFade`).
    if (!dither_keeps(in.fade, in.clip.xy)) {
        discard;
    }
    let layer = in.ids.y;
    let sphere = impostors[layer].center_radius;
    let pixel = in.clip.xy + f32(u32(frame.time.w) % 64u) * vec2<f32>(5.588238, 7.236068);
    let noise = vec2<f32>(interleaved_gradient_noise(pixel), interleaved_gradient_noise(pixel.yx + 17.0));
    let tile = impostor_pick_tile(in.toward, noise);
    var t = impostor_texel(tile, in.plane, in.toward, sphere.w, 0.0);
    var albedo = textureSampleLevel(impostor_albedo, impostor_sampler, t.uv, layer, 0.0);
    var nd = textureSampleLevel(impostor_normal, impostor_sampler, t.uv, layer, 0.0);
    // One parallax step: where the ray meets the surface's height found there, so the tiles
    // blended over a pixel show the same leaf.
    if (t.inside && albedo.a >= 0.5) {
        let refined = impostor_texel(tile, in.plane, in.toward, sphere.w,
            impostor_height(nd.w / albedo.a, sphere.w));
        let a = textureSampleLevel(impostor_albedo, impostor_sampler, refined.uv, layer, 0.0);
        if (refined.inside && a.a >= 0.5) {
            t = refined;
            albedo = a;
            nd = textureSampleLevel(impostor_normal, impostor_sampler, t.uv, layer, 0.0);
        }
    }
    if (!t.inside || albedo.a < 0.5) {
        discard;
    }
    // Edges were blended with the cleared (zero) texels around them: undo that.
    nd /= albedo.a;
    let local = vec2<f32>(t.uv * f32(IMPOSTOR_GRID) - vec2<f32>(tile));
    let xy = vec2<f32>(local.x * 2.0 - 1.0, 1.0 - local.y * 2.0) * sphere.w;
    let p = t.basis[0] * xy.x + t.basis[1] * xy.y + t.basis[2] * impostor_height(nd.w, sphere.w)
        + sphere.xyz;
    var out: ImpostorSurface;
    out.albedo = albedo.rgb / albedo.a;
    out.occlusion = nd.z;
    let n = oct_decode(nd.xy * 2.0 - 1.0);
    out.normal = normalize(in.n0 * n.x + in.n1 * n.y + in.n2 * n.z);
    out.rel = rows_apply(in.m0, in.m1, in.m2, vec4<f32>(p, 1.0));
    let clip = frame.view_proj * vec4<f32>(out.rel, 1.0);
    out.depth = clip.z / clip.w;
    return out;
}

struct ImpostorColor {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_impostor(in: ImpostorOut) -> ImpostorColor {
    let surface = impostor_surface(in);
    let mat = materials[in.ids.x];
    var s = default_surface();
    s.albedo = surface.albedo;
    s.metallic = mat.params.x;
    s.roughness = mat.params.y;
    s.reflectance = mat.params.z;
    s.subsurface = mat.params2.x;
    s.normal = surface.normal;
    s.smooth_normal = surface.normal;
    s.occlusion = surface.occlusion;
    // Far crowns hold snow as their meshes do (mesh.wgsl): on the upper side, where the baked
    // normal faces up.
    if (in.snow > 0.0 && (mat.flags.x & 64u) != 0u) {
        let fallen = saturate(surface_weather(surface.rel).z * SURFACE_SNOW_SCALE_M * 10.0);
        let held = saturate(fallen * in.snow * smoothstep(-0.1, 0.5, surface.normal.y) * 1.6);
        s.albedo = mix(s.albedo, vec3<f32>(0.9, 0.92, 0.95), held);
        s.roughness = mix(s.roughness, 0.75, held);
        s.subsurface = mix(s.subsurface, 0.3, held);
        s.normal = normalize(mix(s.normal, vec3<f32>(0.0, 1.0, 0.0), held * 0.8));
        s.smooth_normal = s.normal;
    }
    var color: vec3<f32>;
    if ((mat.flags.x & 16u) != 0u) {
        color = s.albedo;
    } else if (material_is_thin(mat.flags.x)) {
        color = shade_thin_surface(s, surface.rel, 1.0, surface.normal);
    } else {
        color = shade_surface_shadowed(s, surface.rel, 1.0);
    }
    var out: ImpostorColor;
    out.color = vec4<f32>(apply_fog(color, surface.rel), 1.0);
    out.depth = surface.depth;
    return out;
}

struct ImpostorPrepass {
    @location(0) normal: vec4<f32>,
    @location(1) motion: vec2<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_impostor_prepass(in: ImpostorOut) -> ImpostorPrepass {
    let surface = impostor_surface(in);
    let mat = materials[in.ids.x];
    let p = prepass_output(
        surface.normal,
        mat.params.y,
        motion_clip(surface.rel),
        prev_motion_clip(surface.rel + in.moved),
    );
    var out: ImpostorPrepass;
    out.normal = p.normal;
    out.motion = p.motion;
    out.depth = surface.depth;
    return out;
}
