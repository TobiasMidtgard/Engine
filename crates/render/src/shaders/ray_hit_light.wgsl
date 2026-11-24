// A reflection ray's hit in the frame's ray scene, lit as `shade_surface` lights a surface, for
// every shader that traces the scene for a reflection: after lib_ray.wgsl, with lib_frame. The
// ray pass composes it with lib_ray at group 1 (probe_gi::shader_source, `Rays::World`: every
// pixel's reflection and the mirrors' pictures, ray_reflect.wgsl `reflect_at`); the water's
// bodies with lib_ray at group 3 (ray_scene::water_reflection_lib). probe_gi.wgsl
// `hit_radiance_with` lights a probe's hit as this lights a reflection's, but for its shine, through
// either walk's `trace_scene`: change them together.

// The leaves for the probes' rays and their hits' shadow rays while the canopy map holds no
// crown (`frame.canopy.w` 0: the setting is off, or no crown is near): the crowns as their
// stand-ins' cards and the swaying cards that cast, taken whole by the probes' rays and
// through the holes of their texture by the shadow rays (`SHADOW_RULES`). While the map holds
// crowns, the leaves reach the probes through it (`canopy_ray_transmittance`), never as
// cards.
fn leaves_unless_canopy() -> u32 {
    return select(RAY_LEAF, 0u, frame.canopy.w > 0.0);
}

// What a hit's shadow rays ask of a cut-out card's triangles (a fence, and the leaves while the
// canopy map holds none): their texture, for the first 4 a ray meets, then their coverage, at
// either face, as the shadow maps draw them. The ground stops them from either side, as in the
// probes' own scene.
const SHADOW_RULES: u32 = RULE_LEAF_TEXTURE | (4u << RULE_TESTS_SHIFT);

// What world reflections ask of a cut-out card's triangles: their texture, for the first 4 a
// ray meets, then their coverage, a card drawn from one side only at that side, as the view
// draws it; and the ground only at its front.
const REFLECT_RULES: u32 = RULE_GROUND_FRONT | RULE_LEAF_TEXTURE | RULE_DRAWN_SIDE
    | (4u << RULE_TESTS_SHIFT);

// The lamps a ray's hit asks for their shadow, at most, and the roughest reflecting pixel whose
// hits ask at all (a rougher reflection blurs a lamp's light away; the probes carry it).
const HIT_LAMPS_MOST: u32 = 2u;
const HIT_LAMPS_ROUGHNESS: f32 = 0.3;
// A hit's own shine (`reflected_hit_lit`): on surfaces smoother than this, or metal (a rougher
// dielectric's shine is a few percent of its light, which the reflection's own blur takes).
const HIT_SHINE_ROUGH: f32 = 0.75;
// A hit's highlight is no sharper than this roughness gives: the reflection's pixels average it
// over their lobe, and the sun's glint on a far pane, a texel wide, would only flicker.
const HIT_SHINE_LEAST_ROUGH: f32 = 0.08;
// The ray scene keeps a metal's roughness and how much metal it is, not its colour (its
// diffuse albedo is what isn't metal): its shine is grey, as bright as polished steel's.
const HIT_METAL_F0: f32 = 0.6;

// Whether a shadow ray from camera-relative `origin` along `dir` (unit) gets `t_max` far: what
// casts stops it (`RAY_CASTS`, and the leaves while the canopy map holds none), cut-out cards
// through the holes of their texture (`SHADOW_RULES`), as probe_gi_rays.wgsl
// `trace_scene(.., true)` stops the probes' hits' shadow rays.
fn hit_unshadowed(origin: vec3<f32>, dir: vec3<f32>, t_max: f32) -> bool {
    return ray_stopped(origin, dir, t_max, RAY_CASTS | leaves_unless_canopy(), SHADOW_RULES) == RAY_NONE;
}

// The light a ray's hit `s` at distance `t` along `dir` from `origin` sends back along it (cd/m²,
// not pre-exposed), for a reflecting surface of `roughness` (its lamps, `HIT_LAMPS_ROUGHNESS`):
// the sun and the lamps through shadow rays, the light bounced onto it, its own glow and its own
// shine (`reflected_hit_lit`). Where last frame's probes don't cover the hit (past their grid: a
// far shore, a hill, a ball above a lake), the light bounced onto it is the sky's that
// `shade_surface` guesses there rather than none, which left a reflected thing's shaded side
// black.
fn reflected_hit_light(origin: vec3<f32>, dir: vec3<f32>, t: f32, s: RaySurface, roughness: f32) -> vec3<f32> {
    var p = origin + dir * t;
    if (any(s.n != s.face)) {
        // The shadow rays lift off along `n`: off the face by as much as along a flat face's
        // normal, however far `n` leans.
        p += s.face * (max(0.002, t * 1e-4) * (1.0 - dot(s.n, s.face)));
    }
    let lamps = select(0u, min(HIT_LAMPS_MOST, u32(ray_scene.lights.x)), roughness <= HIT_LAMPS_ROUGHNESS);
    let indirect = probe_gi(p, s.n, -dir);
    var bounced = indirect.rgb * indirect.a;
    if (indirect.a < 1.0) {
        bounced += reflected_sky_guess(s.n, sky_visibility(p, s.n)) * (1.0 - indirect.a);
    }
    return reflected_hit_lit(p, dir, t, s, lamps, bounced);
}

// The light `shade_surface` guesses bounced onto a point with normal `n` where the probes
// don't cover it (irradiance, lux-like): the sky's around `n`, less what the cover over the
// point hides (`sky_vis`, `sky_visibility`: the top-down maps and the indoor volumes), and the
// ambient fill.
fn reflected_sky_guess(n: vec3<f32>, sky_vis: f32) -> vec3<f32> {
    let fill = frame.ambient.rgb;
    return ((sky_ambient_covered(n, sky_vis) - fill) * sky_vis + fill) * PI;
}

// The light leaving camera-relative point `p` of hit `s` back along `dir` (cd/m², not
// pre-exposed): the sun and the nearest `lights_most` lamps through shadow rays, `bounced`
// (irradiance), its own glow, as probe_gi.wgsl `hit_radiance_with` lights a probe's hit; and
// on a smooth or metal hit (`HIT_SHINE_ROUGH`) its own shine, as `shade_surface` gives a
// surface's: the sun's and the lamps' GGX highlights seen along the ray, through the same
// shadow rays, and its reflection of the sky at its roughness, by its Fresnel, dimmed as far as
// `bounced` finds the point darker than the open sky (reflection normalization, Lagarde and
// Zanuttini 2012, as `shade_surface` dims its own: a room's glossy floor in a mirror doesn't
// shine with the sky). The camera's picture holds this shine where it stands for the
// reflection's (ray_reflect.wgsl `full_picture_stands`); lit without it, a glossy floor seen in
// a wall mirror was flat and dark, and a ball over a lake had no highlight in it.
fn reflected_hit_lit(p: vec3<f32>, dir: vec3<f32>, t: f32, s: RaySurface, lights_most: u32, bounced: vec3<f32>) -> vec3<f32> {
    let n = s.n;
    let v = -dir;
    let shine = s.roughness < HIT_SHINE_ROUGH || s.metallic > 0.0;
    let n_dot_v = max(dot(n, v), 1e-4);
    let rough = max(s.roughness, HIT_SHINE_LEAST_ROUGH);
    let alpha = rough * rough;
    let f0 = mix(vec3<f32>(0.04), vec3<f32>(HIT_METAL_F0), saturate(s.metallic));
    let lift = p + n * max(0.002, t * 1e-4);
    // How far shadow rays look: the probes' spacing × 256, as `hit_radiance_with`'s.
    let reach = frame.gi_volume.w * 256.0;
    var light = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    // The sun, through a shadow ray, and through the leaves above (as `hit_radiance_with`).
    let l = frame.sun_dir.xyz;
    let n_dot_l = dot(n, l);
    if (n_dot_l > 0.0 && dot(frame.sun_color.rgb, frame.sun_color.rgb) > 0.0
        && sky_volume_wall(lift, l, reach) == SKY_VOLUME_NO_WALL && hit_unshadowed(lift, l, reach)) {
        let e = frame.sun_color.rgb * sun_transmittance(p) * cloud_shadow(p) * n_dot_l
            * canopy_ray_transmittance(lift, l);
        light += e;
        if (shine) {
            let h = normalize(v + l);
            spec += e * d_ggx(saturate(dot(n, h)), alpha)
                * v_smith_ggx_correlated(n_dot_v, n_dot_l, alpha) * f_schlick(f0, saturate(dot(l, h)));
        }
    }
    // The nearest lamps, through shadow rays.
    let lights = min(frame.counts.x, lights_most);
    for (var i = 0u; i < lights; i++) {
        let pl = point_lights[i];
        let to_light = pl.position_range.xyz - p;
        let dist2 = dot(to_light, to_light);
        let range = pl.position_range.w;
        if (dist2 > range * range) { continue; }
        let ld = to_light * inverseSqrt(max(dist2, 1e-8));
        let nl = dot(n, ld);
        if (nl <= 0.0) { continue; }
        let to_lamp = pl.position_range.xyz - lift;
        let len = length(to_lamp);
        if (len >= 1e-4 && !hit_unshadowed(lift, to_lamp / len, len)) { continue; }
        let falloff = sq(saturate(1.0 - sq(dist2 / (range * range)))) / max(dist2, sq(pl.color_radius.w));
        let e = pl.color_radius.rgb * falloff * nl;
        light += e;
        if (shine) {
            let h = normalize(v + ld);
            spec += e * d_ggx(saturate(dot(n, h)), alpha)
                * v_smith_ggx_correlated(n_dot_v, nl, alpha) * f_schlick(f0, saturate(dot(ld, h)));
        }
    }
    var out = s.albedo * INV_PI * (light + bounced) + s.emissive;
    if (shine) {
        let r = reflect(dir, n);
        let open = luminance(reflected_sky_guess(n, 1.0));
        let sky_share = saturate(luminance(bounced) / max(open, 1e-4));
        out += spec + sample_env_rough(r, rough) * env_brdf_approx(f0, rough, n_dot_v) * sky_share
            * specular_horizon_fade(r, s.face);
    }
    return out;
}
