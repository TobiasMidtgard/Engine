// The screen light pass: the lighting terms that depend on where a surface is and which way it
// faces, worked out once for every pixel the prepass drew, for `shade_surface` to load
// (`screen_light` in lib_frame.wgsl; `RenderSettings::trial_cuts` bit `SCREEN_LIGHT`).
// Requires lib_types + lib_frame + lib_atmosphere. Group 0 is the frame group with the
// prepass's depth and normals bound and stand-ins for the maps drawn after the prepass, so the
// library's own functions run here unchanged.
//
// Why: a pixel's shader runs once for every triangle that touches its 2x2 block. Counted at
// the overview at 1600x900, the ground's shader runs 2.6 times a pixel and meshes' 2.7 times
// (engine-perf/triangle-view), and each run read the shadow maps, marched the depth toward the
// sun, tapped the sky map's rings and blended eight probes. Here that happens once a pixel.
//
// The surface is the prepass's: its normal stands in for the shading normal (as it does for
// ambient occlusion and reflections), and its roughness for the material's in the direction a
// reflection looks for cover.

struct LightOut {
    @location(0) a: vec4<f32>,
    @location(1) b: vec4<f32>,
    @location(2) gi: vec4<f32>,
};

@vertex
fn vs_light(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

@fragment
fn fs_light(in: FullscreenOut) -> LightOut {
    var out: LightOut;
    out.a = vec4<f32>(1.0);
    out.b = vec4<f32>(1.0);
    out.gi = vec4<f32>(0.0);
    let px = vec2<i32>(floor(in.position.xy));
    let z = textureLoad(prepass_depth, px, 0);
    let surface = textureLoad(prepass_normal, px, 0);
    if (z <= 0.0 || surface.w < 0.5) { return out; }
    let uv = (vec2<f32>(px) + 0.5) * frame.viewport.zw;
    let rel = (frame.inv_view * vec4<f32>(view_position(uv, z), 0.0)).xyz;
    let n = oct_decode(surface.xy);
    let v = normalize(-rel);
    let roughness = surface.z * surface.z;

    // The sun: the shadow maps as seen from the side it is on, people's capsules, and the two
    // marches through the depth where anything of it is left.
    let capsules = capsule_occlusion(rel, n);
    if (dot(frame.sun_color.rgb, frame.sun_color.rgb) > 0.0) {
        let side = sunlit_side(n);
        out.a.r = min(shadow_factor(rel, side), capsules.x);
        if (out.a.r > 0.0) {
            out.a.g = contact_shadow(rel, side);
            out.a.b = distant_shadow(rel, side);
        }
    }

    if (shade_skip(SKIP_SCREEN_LIGHT_SUN)) { out.a.r = 0.0; }

    // The sky: cover overhead, leaves, people, and the cover a reflection sees around it.
    out.a.a = sky_visibility(rel, n);
    out.b.r = capsules.y;
    out.b.g = canopy_last_visibility;
    out.b.b = specular_sky_visibility(rel, dominant_reflection(n, v, roughness), n, roughness);

    out.gi = probe_gi(rel, n, v);
    return out;
}
