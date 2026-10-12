// Hair and fur strands, color pass. Requires lib_types + lib_frame + strand_common.

struct StrandOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) tangent: vec3<f32>,
    @location(2) t_shade: vec2<f32>,
    @location(3) @interpolate(flat) groom: u32,
};

@vertex
fn vs_strand(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> StrandOut {
    let p = strand_point(instance, vertex);
    let view = normalize(p.pos + vec3(1e-6));
    let side = normalize(cross(p.tangent, view) + vec3(1e-7));
    // Keep strands at least most of a pixel wide so they don't break up into dots.
    let dist = length(p.pos);
    let pixel = 2.0 * dist / (frame.proj[1][1] * frame.viewport.y);
    let width = max(p.width, pixel * 0.7);
    let s = select(-0.5, 0.5, (vertex & 1u) == 1u);
    let rel = p.pos + side * width * s;
    var out: StrandOut;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.rel = rel;
    out.tangent = p.tangent;
    out.t_shade = vec2<f32>(p.t, p.shade);
    out.groom = p.groom;
    return out;
}

// Kajiya-Kay strand lighting with two shifted specular lobes (a white primary reflection and
// a colored secondary one through the fiber), a little self-shadowing toward the root.
@fragment
fn fs_strand(in: StrandOut) -> @location(0) vec4<f32> {
    let g = grooms[in.groom];
    let albedo = g.color.rgb * in.t_shade.y;
    let t = safe_normalize(in.tangent, vec3(0.0, 1.0, 0.0));
    let v = normalize(-in.rel);
    let n = safe_normalize(v - t * dot(v, t), safe_normalize(cross(t, vec3(0.0, 0.0, 1.0)), vec3(1.0, 0.0, 0.0)));
    let l = frame.sun_dir.xyz;
    let sun = frame.sun_color.rgb * sun_transmittance(in.rel);
    var color = vec3(0.0);
    if (dot(sun, sun) > 0.0) {
        let shadow = shadow_factor(in.rel, n);
        let tl = dot(t, l);
        let diffuse = sqrt(max(1.0 - tl * tl, 0.0)) * saturate(dot(n, l) * 0.5 + 0.6);
        let h = safe_normalize(l + v, n);
        let t1 = normalize(t + n * 0.1);
        let t2 = normalize(t - n * 0.15);
        let th1 = dot(t1, h);
        let th2 = dot(t2, h);
        let spec1 = pow(sqrt(max(1.0 - th1 * th1, 0.0)), 90.0) * 0.06;
        let spec2 = pow(sqrt(max(1.0 - th2 * th2, 0.0)), 24.0) * 0.12;
        color += sun * shadow * (albedo * diffuse * INV_PI + vec3(spec1) + albedo * spec2);
    }
    let sky = sky_ambient(n) * sky_visibility(in.rel, n) * PI;
    color += sky * albedo * INV_PI * 0.8;
    // Strands near the root sit deep in the hair and get less light (less so in short fur).
    color *= mix(g.extra.y, 1.0, sqrt(saturate(in.t_shade.x)));
    if (frame.counts.z == 1u) {
        return vec4<f32>(t * 0.5 + 0.5, 1.0);
    }
    return vec4<f32>(apply_fog(pre_expose(color), in.rel), 1.0);
}
