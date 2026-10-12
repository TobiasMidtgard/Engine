//! The ground of the range: the concrete pad (cast in slabs with expansion joints, stained
//! and scorched), the painted lane markings, soot decals, and the bullet trap at the far end
//! (an earth berm behind a timber wall on steel H-posts).

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::art::{self, Parts};
use super::grass::Spot;
use super::kit::{self, at, tr};
use super::tex::{noise, smoothstep};

/// Height of the pad's top surface, m.
pub const PAD_TOP: f32 = 0.10;
/// Z of the timber wall's front face.
pub const WALL_Z: f32 = 19.95;
/// Height of the timber wall.
pub const WALL_H: f32 = 3.05;

/// Smooth world-space noise, about -1..1 (does not tile).
pub fn wnoise(x: f32, z: f32, scale: f32, seed: u32) -> f32 {
    noise(x / scale + 512.0, z / scale + 512.0, 4096, 4096, seed)
}

/// Two octaves of [`wnoise`].
pub fn wfbm(x: f32, z: f32, scale: f32, seed: u32) -> f32 {
    wnoise(x, z, scale, seed) * 0.65 + wnoise(x, z, scale * 0.4, seed + 1) * 0.35
}

/// Large-scale stains on the pad (vertex color multiplier) at a point on its surface.
fn pad_stain(p: Vec3, half: [f32; 2], blast: Vec2, spots: &[(Vec2, f32, f32)]) -> Vec3 {
    let (x, z) = (p.x, p.z);
    let mut c = Vec3::ONE;
    // Weathering blotches.
    let n = wfbm(x, z, 7.0, 101);
    c *= 1.0 - smoothstep(-0.1, 0.6, n) * 0.13;
    let n2 = wfbm(x, z, 2.5, 103);
    c *= 1.0 + n2 * 0.04;
    // Dust blown off the berm and along the sides.
    let dust = smoothstep(14.0, 21.5, z) * 0.8
        + smoothstep(half[0] - 2.5, half[0], x.abs()) * 0.5
        + smoothstep(half[1] - 2.0, half[1], -z) * 0.3;
    let dust = (dust * (0.7 + 0.3 * wnoise(x, z, 1.5, 107))).clamp(0.0, 1.0);
    c = c.lerp(c * Vec3::new(0.92, 0.84, 0.72), dust);
    // Soot around the blast point and the grenade spots.
    let d = Vec2::new(x, z).distance(blast) + wfbm(x, z, 1.2, 109) * 1.1;
    let soot = smoothstep(5.5, 0.8, d);
    c = c.lerp(Vec3::new(0.38, 0.36, 0.34), soot * 0.7);
    for &(g, r, dark) in spots {
        let d = Vec2::new(x, z).distance(g) + wnoise(x, z, 0.6, 113) * 0.3 * r;
        c *= 1.0 - smoothstep(r, r * 0.15, d) * dark;
    }
    // Rain tracks: slightly darker streaks across the pad.
    let streak = wnoise(x * 0.3, z * 3.0, 1.0, 117);
    c *= 1.0 - smoothstep(0.4, 0.8, streak) * 0.05;
    c
}

/// The pad: slabs with joints, sealant showing in the joints, stains baked into the slabs'
/// vertex colors.
pub fn pad(parts: &mut Parts, half: [f32; 2], blast: Vec2, spots: &[(Vec2, f32, f32)]) {
    let (nx, nz) = (8, 10);
    let (sx, sz) = (half[0] * 2.0 / nx as f32, half[1] * 2.0 / nz as f32);
    let joint = 0.014;
    let depth = 0.16;
    let mut rng = Rng::new(0x5_1AB);
    for j in 0..nz {
        for i in 0..nx {
            let cx = -half[0] + sx * (i as f32 + 0.5);
            let cz = -half[1] + sz * (j as f32 + 0.5);
            let mut slab =
                kit::rounded_box(Vec3::new(sx - joint, depth, sz - joint), 0.012, 2, 0.5);
            slab = kit::offset_uv(slab, Vec2::new(rng.range(0.0, 3.0), rng.range(0.0, 3.0)));
            let tone = 1.0 + rng.range(-0.05, 0.05);
            let warm = rng.range(-0.02, 0.02);
            let slab = kit::placed(slab, at(Vec3::new(cx, PAD_TOP - depth * 0.5, cz)));
            let mut slab = kit::tint(slab, Vec3::new(tone + warm, tone, tone - warm));
            // Each slab is a little darker toward its joints (dirt and curing).
            let hx = sx * 0.5;
            let hz = sz * 0.5;
            kit::shade(&mut slab, |p, n| {
                let e = (hx - (p.x - cx).abs()).min(hz - (p.z - cz).abs());
                let edge = 1.0 - (1.0 - smoothstep(0.0, 0.5, e)) * 0.08;
                let side = if n.y < 0.5 { 0.8 } else { 1.0 };
                pad_stain(p, half, blast, spots) * edge * side
            });
            parts.add(art::CONCRETE, slab);
        }
    }
    // Joint sealant: a dark sheet just under the surface, seen only through the joints.
    let sheet = kit::rounded_box(
        Vec3::new(half[0] * 2.0 - 0.01, 0.004, half[1] * 2.0 - 0.01),
        0.0,
        1,
        4.0,
    );
    parts.add_at(art::RUBBER, sheet, at(Vec3::new(0.0, PAD_TOP - 0.012, 0.0)));
}

/// A flat painted strip from `a` to `b` (on the pad), `w` wide.
fn strip(a: Vec2, b: Vec2, w: f32, y: f32) -> Mesh {
    let d = (b - a).normalize_or(Vec2::X);
    let side = Vec2::new(-d.y, d.x) * (w * 0.5);
    let len = (b - a).length();
    let steps = (len / 1.0).ceil().max(1.0) as u32;
    let mut m = Mesh::default();
    for k in 0..=steps {
        let t = k as f32 / steps as f32;
        let c = a + (b - a) * t;
        for s in [-1.0f32, 1.0] {
            let p = c + side * s;
            m.vertices.push(kit::vtx(
                Vec3::new(p.x, y, p.y),
                Vec3::Y,
                Vec2::new(p.x, p.y),
            ));
        }
    }
    for k in 0..steps {
        let i = k * 2;
        // Counter-clockwise seen from above.
        let (p0, p1) = (
            m.vertices[i as usize].position,
            m.vertices[i as usize + 2].position,
        );
        let (q0, _) = (m.vertices[i as usize + 1].position, ());
        let e1 = Vec3::from(p1) - Vec3::from(p0);
        let e2 = Vec3::from(q0) - Vec3::from(p0);
        if e1.cross(e2).y > 0.0 {
            m.indices
                .extend_from_slice(&[i, i + 2, i + 1, i + 1, i + 2, i + 3]);
        } else {
            m.indices
                .extend_from_slice(&[i, i + 1, i + 2, i + 1, i + 3, i + 2]);
        }
    }
    m
}

/// Clip a convex polygon to the half-plane `n · p <= d`.
fn clip(poly: &[Vec2], n: Vec2, d: f32) -> Vec<Vec2> {
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (da, db) = (n.dot(a) - d, n.dot(b) - d);
        if da <= 0.0 {
            out.push(a);
        }
        if (da < 0.0) != (db < 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

fn flat_poly(poly: &[Vec2], y: f32) -> Mesh {
    let mut m = Mesh::default();
    if poly.len() < 3 {
        return m;
    }
    // Polygons are counter-clockwise in (x, z); seen from above (+Y) that is clockwise, so
    // flip the fan.
    for p in poly {
        m.vertices
            .push(kit::vtx(Vec3::new(p.x, y, p.y), Vec3::Y, *p));
    }
    for k in 1..poly.len() as u32 - 1 {
        m.indices.extend_from_slice(&[0, k + 1, k]);
    }
    if kit::area(poly) < 0.0 {
        for t in m.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
    }
    m
}

/// Lane markings: the yellow firing line, white lane dividers (broken where the track
/// crosses), distance bars, and a hatched keep-out square around the steam vent.
pub fn markings(parts: &mut Parts, vent: Vec2, track_z: [f32; 2]) {
    let y = PAD_TOP + 0.0015;
    parts.add(
        art::LINE_YELLOW,
        strip(Vec2::new(-16.0, -17.35), Vec2::new(16.0, -17.35), 0.22, y),
    );
    let keep_out = 1.75;
    for x in [-15.0f32, -9.0, -3.0, 3.0, 9.0, 15.0] {
        for (z0, z1) in [(-17.1, track_z[0]), (track_z[1], 14.8)] {
            // Broken around the vent's keep-out square.
            let mut runs = vec![(z0, z1)];
            if (x - vent.x).abs() < keep_out && vent.y > z0 && vent.y < z1 {
                runs = vec![(z0, vent.y - keep_out), (vent.y + keep_out, z1)];
            }
            for (a, b) in runs {
                parts.add(
                    art::LINE_WHITE,
                    strip(Vec2::new(x, a), Vec2::new(x, b), 0.1, y),
                );
            }
        }
    }
    for z in [-7.35f32, 2.65, 12.65] {
        for lane in [-12.0f32, -6.0, 0.0, 6.0, 12.0] {
            parts.add(
                art::LINE_WHITE,
                strip(Vec2::new(lane - 0.7, z), Vec2::new(lane + 0.7, z), 0.12, y),
            );
        }
    }
    // Keep-out square: border and diagonal hatching.
    let h = 1.6;
    let sq = [
        vent + Vec2::new(-h, -h),
        vent + Vec2::new(h, -h),
        vent + Vec2::new(h, h),
        vent + Vec2::new(-h, h),
    ];
    for i in 0..4 {
        let (a, b) = (sq[i], sq[(i + 1) % 4]);
        let d = (b - a).normalize();
        parts.add(art::LINE_YELLOW, strip(a - d * 0.06, b + d * 0.06, 0.12, y));
    }
    let diag = Vec2::new(1.0, 1.0).normalize();
    let mut c = -h * 1.5;
    while c < h * 1.5 {
        let c0 = vent.dot(diag) + c;
        let mut poly = sq.to_vec();
        poly = clip(&poly, diag, c0 + 0.09);
        poly = clip(&poly, -diag, -(c0 - 0.09));
        // Leave the middle clear around the pipe.
        if poly.len() >= 3 {
            parts.add(art::LINE_YELLOW, flat_poly(&poly, y));
        }
        c += 0.42;
    }
}

/// A square decal (`mat`: soot, oil) of radius `r` lying on the pad at `p`.
pub fn decal(parts: &mut Parts, mat: &'static str, p: Vec2, r: f32, angle: f32, lift: f32) {
    let mut m = Mesh::default();
    let rot = Quat::from_rotation_y(angle);
    for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        let local = rot * Vec3::new((u - 0.5) * 2.0 * r, 0.0, (v - 0.5) * 2.0 * r);
        m.vertices.push(kit::vtx(
            Vec3::new(p.x, PAD_TOP + lift, p.y) + local,
            Vec3::Y,
            Vec2::new(u, v),
        ));
    }
    // (0,0) -> (1,0) -> (1,1) runs clockwise seen from above; flip to face up.
    m.indices.extend_from_slice(&[0, 2, 1, 0, 3, 2]);
    parts.add(mat, m);
}

/// Where the shoulder runs: points along the pad's edge (sides and near end) with their
/// outward normals, about `step` apart, turning round the two near corners.
fn shoulder_path(half: [f32; 2], step: f32) -> Vec<(Vec2, Vec2)> {
    let [hx, hz] = half;
    let mut path = Vec::new();
    let line = |path: &mut Vec<(Vec2, Vec2)>, a: Vec2, b: Vec2, n: Vec2| {
        let k = ((b - a).length() / step).ceil().max(1.0) as u32;
        for i in 0..k {
            path.push((a.lerp(b, i as f32 / k as f32), n));
        }
    };
    let corner = |path: &mut Vec<(Vec2, Vec2)>, c: Vec2, a0: f32| {
        for i in 0..8 {
            let a = a0 + i as f32 / 8.0 * std::f32::consts::FRAC_PI_2;
            path.push((c, Vec2::new(a.cos(), a.sin())));
        }
    };
    line(
        &mut path,
        Vec2::new(-hx, hz),
        Vec2::new(-hx, -hz),
        Vec2::NEG_X,
    );
    corner(&mut path, Vec2::new(-hx, -hz), std::f32::consts::PI);
    line(
        &mut path,
        Vec2::new(-hx, -hz),
        Vec2::new(hx, -hz),
        Vec2::NEG_Y,
    );
    corner(&mut path, Vec2::new(hx, -hz), -std::f32::consts::FRAC_PI_2);
    line(&mut path, Vec2::new(hx, -hz), Vec2::new(hx, hz), Vec2::X);
    path.push((Vec2::new(hx, hz), Vec2::X));
    path
}

/// Width of the shoulder at a point of its path.
fn shoulder_width(p: Vec2) -> f32 {
    2.2 + wfbm(p.x, p.y, 3.0, 401) * 0.9
}

/// Height of the shoulder `s` (0 at the pad, 1 at its outer edge) of the way out.
fn shoulder_y(p: Vec2, s: f32) -> f32 {
    let top = PAD_TOP - 0.04;
    let low = -0.04;
    top + (low - top) * smoothstep(0.0, 1.0, s) + wnoise(p.x, p.y, 0.5, 403) * 0.012 * s.min(1.0)
}

/// The gravel shoulder around the pad's sides and near end (the berm closes the far end): it
/// slopes from just under the pad's edge down into the ground, with a ragged outer edge.
/// Returns spots for grass tufts along it.
pub fn shoulder(parts: &mut Parts, half: [f32; 2]) -> Vec<Spot> {
    let path = shoulder_path(half, 0.35);
    let n = path.len() as u32 - 1;
    let ns = 6;
    let at_path = |u: f32| path[((u * n as f32).round() as usize).min(path.len() - 1)];
    let mut m = kit::surface(n, ns, false, |u, s| {
        let (p, nrm) = at_path(u);
        let w = shoulder_width(p + nrm);
        let q = p + nrm * (w * s + 0.004);
        let y = shoulder_y(q, s);
        (Vec3::new(q.x, y, q.y), q)
    });
    // Darker, finer grit at the outer edge where it meets the soil.
    kit::shade(&mut m, |p, _| {
        let n = wnoise(p.x, p.z, 0.8, 405);
        Vec3::splat((0.85 + 0.15 * n) * (1.0 - smoothstep(0.03, -0.03, p.y) * 0.25))
    });
    parts.add(art::GRAVEL_FINE, m);

    // Grass along the outer edge (thick) and creeping in (sparse).
    let mut rng = Rng::new(0x6_4A55);
    let mut spots = Vec::new();
    for &(p, nrm) in &path {
        let w = shoulder_width(p + nrm);
        for _ in 0..7 {
            let s = 1.25 - rng.range(0.0, 1.0f32).powf(2.2) * 0.85;
            let along = Vec2::new(-nrm.y, nrm.x) * rng.range(-0.2, 0.2);
            let q = p + nrm * (w * s) + along;
            let y = if s < 1.0 { shoulder_y(q, s) } else { -0.02 };
            let size = rng.range(0.6, 1.2) * (0.55 + 0.5 * s.min(1.0));
            spots.push((Vec3::new(q.x, y, q.y), size, smoothstep(0.5, 1.2, s)));
        }
    }
    spots
}

/// Weeds in the pad's expansion joints (more toward the edges) and along the foot of the
/// timber wall.
pub fn weeds(half: [f32; 2], seed: u64) -> Vec<Spot> {
    let [hx, hz] = half;
    let (nx, nz) = (8, 10);
    let (sx, sz) = (hx * 2.0 / nx as f32, hz * 2.0 / nz as f32);
    let mut rng = Rng::new(seed);
    let mut spots = Vec::new();
    while spots.len() < 70 {
        let (x, z) = if rng.f32() < 0.5 {
            (
                -hx + sx * rng.range_i32(1, nx) as f32,
                rng.range(-hz + 0.3, WALL_Z - 0.3),
            )
        } else {
            (
                rng.range(-hx + 0.3, hx - 0.3),
                -hz + sz * rng.range_i32(1, nz) as f32,
            )
        };
        let edge = smoothstep(9.0, 18.0, x.abs()).max(smoothstep(4.0, 20.0, -z));
        if rng.f32() > 0.15 + edge * 0.85 {
            continue;
        }
        spots.push((Vec3::new(x, PAD_TOP, z), rng.range(0.35, 0.75), 0.5));
    }
    for _ in 0..55 {
        let x = rng.range(-hx + 0.4, hx - 0.4);
        spots.push((
            Vec3::new(x, PAD_TOP, WALL_Z - rng.range(0.02, 0.18)),
            rng.range(0.4, 0.95),
            0.3,
        ));
    }
    spots
}

/// The bullet trap: an earth berm behind a timber wall on steel H-posts, across the far end
/// of the pad (x in ±`half_x`).
pub fn berm(parts: &mut Parts, half_x: f32, targets: &[f32]) {
    let mut rng = Rng::new(0xBE_A3);
    // H-posts every 2.4 m.
    let post_h = WALL_H + 0.18;
    let flange = 0.2;
    let (tf, tw) = (0.016, 0.011);
    let hprof = vec![
        Vec2::new(-flange / 2.0, -flange / 2.0),
        Vec2::new(flange / 2.0, -flange / 2.0),
        Vec2::new(flange / 2.0, -flange / 2.0 + tf),
        Vec2::new(tw / 2.0, -flange / 2.0 + tf),
        Vec2::new(tw / 2.0, flange / 2.0 - tf),
        Vec2::new(flange / 2.0, flange / 2.0 - tf),
        Vec2::new(flange / 2.0, flange / 2.0),
        Vec2::new(-flange / 2.0, flange / 2.0),
        Vec2::new(-flange / 2.0, flange / 2.0 - tf),
        Vec2::new(-tw / 2.0, flange / 2.0 - tf),
        Vec2::new(-tw / 2.0, -flange / 2.0 + tf),
        Vec2::new(-flange / 2.0, -flange / 2.0 + tf),
    ];
    // Profile in (x, -z): the flanges face front and back. Extruded along z, then stood up.
    let post = kit::extrude(&kit::fillet(&hprof, 0.004, 1), 0.0, post_h, 30.0, 0.004);
    let wall_mid = WALL_Z + 0.1;
    let bays = ((half_x * 2.0) / 2.4).round() as i32;
    let span = 2.4;
    let x0 = -span * bays as f32 / 2.0;
    for k in 0..=bays {
        let x = x0 + span * k as f32;
        // Extruded along +Z: rotate so +Z becomes +Y.
        let t = tr(
            Vec3::new(x, PAD_TOP - 0.05, wall_mid),
            Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        );
        let mut m = kit::placed(
            kit::offset_uv(post.clone(), Vec2::new(rng.f32(), rng.f32())),
            t,
        );
        kit::ground_grime(&mut m, PAD_TOP, 0.5, 0.6, Vec3::new(0.9, 0.8, 0.65));
        parts.add(art::RUST, m);
        // Base plate and anchor bolts.
        let plate = kit::rounded_box(Vec3::new(0.34, 0.025, 0.34), 0.006, 1, 0.5);
        parts.add_at(
            art::RUST,
            plate,
            at(Vec3::new(x, PAD_TOP + 0.012, wall_mid - 0.08)),
        );
        for (dx, dz) in [(-0.12, -0.2), (0.12, -0.2)] {
            parts.add_at(
                art::GALV,
                kit::hex_bolt(0.022),
                at(Vec3::new(x + dx, PAD_TOP + 0.025, wall_mid + dz + 0.08)),
            );
        }
    }
    // Railroad ties stacked between the posts.
    let courses = 14;
    let course_h = (WALL_H - PAD_TOP) / courses as f32;
    for k in 0..bays {
        let xc = x0 + span * (k as f32 + 0.5);
        for c in 0..courses {
            let h = course_h - rng.range(0.004, 0.012);
            let depth = 0.165;
            let len = span - 0.02;
            let mut tie = kit::rounded_box(Vec3::new(len, h, depth), 0.018, 2, 0.6);
            tie = kit::offset_uv(tie, Vec2::new(rng.range(0.0, 1.2), rng.range(0.0, 1.2)));
            let y = PAD_TOP + course_h * (c as f32 + 0.5);
            let t = tr(
                Vec3::new(
                    xc + rng.range(-0.01, 0.01),
                    y,
                    wall_mid + rng.range(-0.012, 0.012),
                ),
                Quat::from_rotation_z(rng.range(-0.003, 0.003))
                    * Quat::from_rotation_y(rng.range(-0.004, 0.004)),
            );
            let tone = rng.range(0.8, 1.12);
            let mut m = kit::tint(
                kit::placed(tie, t),
                Vec3::new(tone, tone * 0.98, tone * 0.95),
            );
            // Shot-up, lead-smeared patches behind the targets; darker toward the bottom.
            kit::shade(&mut m, |p, _| {
                let mut f = 1.0;
                for &tx in targets {
                    let d = Vec2::new((p.x - tx) / 1.6, (p.y - 1.8) / 1.0).length()
                        + wnoise(p.x, p.y, 0.4, 211) * 0.3;
                    f *= 1.0 - smoothstep(1.0, 0.2, d) * 0.4;
                }
                Vec3::splat(f)
            });
            kit::ground_grime(&mut m, PAD_TOP, 0.6, 0.55, Vec3::new(0.9, 0.82, 0.7));
            parts.add(art::TIMBER, m);
        }
    }
    // Cap beam on top of the wall.
    let cap = kit::rounded_box(Vec3::new(span * bays as f32 + 0.3, 0.12, 0.3), 0.02, 2, 0.8);
    parts.add_at(
        art::TIMBER,
        kit::tint(cap, Vec3::splat(0.85)),
        at(Vec3::new(0.0, WALL_H + 0.06, wall_mid + 0.04)),
    );

    // The earth berm: a noisy ridge behind the wall, tapering at the ends.
    let earth = berm_surface(half_x);
    parts.add(art::EARTH, earth);
}

/// Ridge profile (z, y) of the berm, behind the wall; `spill` (0..1) pulls its front down to
/// the ground where there is no wall.
fn berm_profile(s: f32, spill: f32) -> Vec2 {
    let keys = [
        (0.0, Vec2::new(WALL_Z + 0.25, PAD_TOP - 0.1)),
        (0.12, Vec2::new(WALL_Z + 0.25, WALL_H - 0.05)),
        (0.2, Vec2::new(WALL_Z + 0.6, WALL_H + 0.08)),
        (0.45, Vec2::new(WALL_Z + 2.7, 5.2)),
        (0.58, Vec2::new(WALL_Z + 3.7, 5.65)),
        (0.7, Vec2::new(WALL_Z + 4.8, 5.3)),
        (1.0, Vec2::new(WALL_Z + 8.5, -0.15)),
    ];
    let spill_keys = [
        (0.0, Vec2::new(WALL_Z - 3.2, PAD_TOP - 0.15)),
        (0.12, Vec2::new(WALL_Z - 1.5, 1.4)),
        (0.2, Vec2::new(WALL_Z - 0.5, 2.6)),
    ];
    let pick = |keys: &[(f32, Vec2)]| -> Vec2 {
        for w in keys.windows(2) {
            let (a, b) = (w[0], w[1]);
            if s <= b.0 {
                let t = ((s - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
                let t = t * t * (3.0 - 2.0 * t) * 0.6 + t * 0.4;
                return a.1.lerp(b.1, t);
            }
        }
        keys[keys.len() - 1].1
    };
    let p = pick(&keys);
    if s <= 0.2 && spill > 0.0 {
        p.lerp(pick(&spill_keys), spill)
    } else {
        p
    }
}

/// A point of the berm's surface: `u` across (from +x to -x, so the surface faces the
/// range), `s` from the foot of the wall over the crest to the back.
fn berm_point(half_x: f32, u: f32, s: f32) -> Vec3 {
    let ext = half_x + 3.5;
    let x = ext - u * ext * 2.0;
    let ax = x.abs();
    // Beyond the wall the earth spills forward and the ridge drops to the ground.
    let spill = smoothstep(half_x - 0.3, half_x + 1.2, ax);
    let end = smoothstep(ext, half_x + 0.4, ax);
    let mut p = berm_profile(s, spill);
    let ground = PAD_TOP - 0.15;
    p.y = ground + (p.y - ground) * (0.15 + 0.85 * end);
    // Lumps and erosion gullies (down the slopes), but none behind the wall.
    let free = smoothstep(0.1, 0.22, s).max(spill);
    let lump = wfbm(x, p.x, 2.5, 301) * 0.28 + wnoise(x, p.x, 0.7, 303) * 0.08;
    let gully = (1.0 - wnoise(x, p.y * 0.15, 1.3, 307).abs()).powf(6.0) * 0.25;
    p.y += (lump - gully) * free * end.max(0.3);
    let z = p.x + wnoise(x, p.y, 1.8, 309) * 0.25 * free;
    Vec3::new(x, p.y.max(ground), z)
}

/// How grassy the berm is at `p` (normal `n`): dry grass and weeds on the crest and the back,
/// bare churned earth on the face behind the targets.
fn berm_grass(p: Vec3, n: Vec3) -> f32 {
    let grass = smoothstep(0.55, 0.85, n.y)
        * smoothstep(3.8, 5.2, p.y)
        * (0.6 + 0.4 * wnoise(p.x, p.z, 1.5, 311));
    let grass = grass.max(smoothstep(23.5, 25.5, p.z) * (0.5 + 0.5 * wnoise(p.x, p.z, 2.0, 313)));
    let patchy = smoothstep(-0.2, 0.4, wfbm(p.x, p.z, 1.2, 317));
    (grass * patchy).clamp(0.0, 1.0)
}

fn berm_surface(half_x: f32) -> Mesh {
    let ext = half_x + 3.5;
    let nx = (ext * 2.0 / 0.45) as u32;
    let ns = 44;
    let mut m = kit::surface(nx, ns, false, |u, s| {
        let p = berm_point(half_x, u, s);
        (p, Vec2::new(p.x, p.z + p.y))
    });
    // Dry grass ground cover where the tufts grow, darker damp earth at the foot.
    kit::shade(&mut m, |p, n| {
        let g = berm_grass(p, n);
        let dry = Vec3::new(1.2, 1.12, 0.78);
        let mut c = Vec3::ONE.lerp(dry, g);
        c *= 1.0 - smoothstep(1.2, PAD_TOP, p.y) * 0.25;
        c * (0.9 + 0.1 * wnoise(p.x, p.z, 0.5, 319))
    });
    m
}

/// Grass tufts on the berm: spots (root, size).
pub fn berm_tufts(half_x: f32, seed: u64) -> Vec<Spot> {
    let mut rng = Rng::new(seed);
    let mut spots = Vec::new();
    for _ in 0..12000 {
        let (u, s) = (rng.f32(), rng.range(0.15, 1.0));
        let p = berm_point(half_x, u, s);
        let du = berm_point(half_x, (u + 0.002).min(1.0), s)
            - berm_point(half_x, (u - 0.002).max(0.0), s);
        let ds = berm_point(half_x, u, (s + 0.004).min(1.0))
            - berm_point(half_x, u, (s - 0.004).max(0.0));
        let n = du.cross(ds).normalize_or(Vec3::Y);
        let g = berm_grass(p, n).max(0.05);
        if rng.f32() < g * 0.85 {
            spots.push((p, rng.range(0.75, 1.6) * (0.65 + 0.55 * g), 0.25));
        }
    }
    spots
}
