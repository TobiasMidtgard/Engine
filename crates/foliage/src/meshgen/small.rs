//! Small plants: fern clumps, grass tufts, flowers and mushroom clusters.

use engine_core::math::{Quat, Rng, Vec3, Vec4};

use super::tree::PlantMeshes;
use super::{MeshBuilder, bezier, rgb};
use crate::species::{PlantForm, PlantSpecies};

/// Build one variant of a small plant.
pub fn small(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut m = match sp.form {
        PlantForm::Fern => fern(sp, seed),
        PlantForm::Flower => flowers(sp, seed),
        PlantForm::Mushroom => mushrooms(sp, seed),
        _ => grass(sp, seed),
    };
    m.share_bounds();
    m
}

fn finish_foliage(b: MeshBuilder, _top: f32) -> engine_render::Mesh {
    b.finish()
}

fn fern(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let mut b = MeshBuilder::new();
    let n = sh.branches as i32 + rng.range_i32(-2, 3);
    let leaf = Vec3::from(sp.leaf_color).max(Vec3::splat(0.01));
    let accent = Vec3::from(sp.accent_color) / leaf;
    for k in 0..n.max(4) {
        let az = k as f32 / n as f32 * std::f32::consts::TAU + rng.range(-0.3, 0.3);
        let dir = Vec3::new(az.cos(), 0.0, az.sin());
        let side = Vec3::new(-az.sin(), 0.0, az.cos());
        let tilt = rng.range(sh.angle_high_deg, sh.angle_low_deg).to_radians();
        let length = sp.crown_radius_m * rng.range(0.8, 1.25) * 1.3;
        let rise = h * rng.range(0.75, 1.05) * tilt.cos().max(0.35);
        let p0 = Vec3::Y * 0.02;
        let p1 = dir * length * 0.35 + Vec3::Y * rise;
        let p2 = dir * length + Vec3::Y * rise * rng.range(0.15, 0.45);
        let width = length * 0.42;
        let segs = 8;
        let tint = Vec3::ONE.lerp(accent, rng.range(0.0, 0.5)) * rng.range(0.85, 1.1);
        let (mut l, mut r, mut nrm, mut v, mut col) = (vec![], vec![], vec![], vec![], vec![]);
        for i in 0..=segs {
            let t = i as f32 / segs as f32;
            let p = bezier(p0, p1, p2, t);
            let tangent = (bezier(p0, p1, p2, (t + 0.02).min(1.0))
                - bezier(p0, p1, p2, (t - 0.02).max(0.0)))
            .normalize_or(dir);
            // The frond faces up, rolled a little so it doesn't look flat-on from above.
            let s = Quat::from_axis_angle(tangent, rng.range(-0.25, 0.25)) * side;
            l.push(p - s * width * 0.5);
            r.push(p + s * width * 0.5);
            nrm.push((Vec3::Y * 0.7 + dir * 0.3).normalize());
            v.push(1.0 - t);
            col.push((tint * (0.55 + 0.45 * t)).extend(1.0));
        }
        b.strip(&l, &r, &nrm, &v, &col);
    }
    PlantMeshes {
        foliage: Some(finish_foliage(b, h * 1.1)),
        height: h * 1.1,
        ..Default::default()
    }
}

fn grass(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let mut b = MeshBuilder::new();
    let leaf = Vec3::from(sp.leaf_color).max(Vec3::splat(0.01));
    let dry = Vec3::from(sp.accent_color) / leaf;
    let blades = sh.branches.max(6);
    for _ in 0..blades {
        let base2 = rng.in_unit_disk() * sp.crown_radius_m * 0.9;
        let base = Vec3::new(base2.x, 0.0, base2.y);
        let height = h * rng.range(0.4, 1.0);
        let az = rng.range(0.0, std::f32::consts::TAU);
        let out = (base + Vec3::new(az.cos(), 0.0, az.sin()) * 0.3).normalize_or(Vec3::X);
        let lean = height * rng.range(0.15, 0.6) * (1.0 + sh.angle_low_deg / 90.0);
        let width = 0.008 + sp.trunk_radius_m * rng.range(2.0, 4.5);
        let side = Vec3::new(-out.z, 0.0, out.x);
        let twist = Quat::from_rotation_y(rng.range(-0.6, 0.6));
        let side = twist * side;
        let tip_dry = if rng.chance(0.3) {
            rng.range(0.3, 1.0)
        } else {
            0.0
        };
        let shade = rng.range(0.8, 1.1);
        let segs = 4;
        let (mut l, mut r, mut nrm, mut v, mut col) = (vec![], vec![], vec![], vec![], vec![]);
        for i in 0..=segs {
            let t = i as f32 / segs as f32;
            let p = base + Vec3::Y * height * t + out * lean * t * t;
            let w = width * (1.0 - t).powf(0.7) * 0.5;
            l.push(p - side * w);
            r.push(p + side * w);
            nrm.push((Vec3::Y * 0.7 + out * 0.3).normalize());
            v.push(1.0 - t);
            let c = Vec3::ONE.lerp(dry, tip_dry * t * t) * shade * (0.3 + 0.75 * t);
            col.push(c.extend(1.0));
        }
        b.strip(&l, &r, &nrm, &v, &col);
    }
    PlantMeshes {
        foliage: Some(finish_foliage(b, h * 1.05)),
        height: h * 1.05,
        ..Default::default()
    }
}

/// Flowers (and their leaves) take their colors from vertex colors; the material is white.
fn flowers(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let mut b = MeshBuilder::new();
    let stem = Vec3::from(sp.bark_color);
    let petal = Vec3::from(sp.leaf_color);
    let accent = Vec3::from(sp.accent_color);
    let bells = sh.tip_bend < -0.3;
    // A few leaves at the base.
    for _ in 0..rng.range_i32(3, 6) {
        let az = rng.range(0.0, std::f32::consts::TAU);
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let side = Vec3::new(-az.sin(), 0.0, az.cos());
        let len = h * rng.range(0.35, 0.6);
        let (mut l, mut r, mut nrm, mut v, mut col) = (vec![], vec![], vec![], vec![], vec![]);
        for i in 0..=3 {
            let t = i as f32 / 3.0;
            let p = out * len * t * 0.8 + Vec3::Y * len * (t * 0.9 - t * t * 0.5);
            let w = 0.025 * (std::f32::consts::PI * t.max(0.15)).sin();
            l.push(p - side * w);
            r.push(p + side * w);
            nrm.push(Vec3::Y);
            v.push(1.0 - t);
            col.push((stem * (0.6 + 0.4 * t)).extend(1.0));
        }
        b.strip(&l, &r, &nrm, &v, &col);
    }
    let n = sh.branches as i32 + rng.range_i32(-2, 3);
    for _ in 0..n.max(3) {
        let base2 = rng.in_unit_disk() * sp.crown_radius_m * 0.8;
        let base = Vec3::new(base2.x, 0.0, base2.y);
        let height = h * rng.range(0.55, 1.0);
        let lean = Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)) * height * 0.15
            + base.normalize_or(Vec3::X) * height * if bells { 0.35 } else { 0.08 };
        let top = base + Vec3::Y * height + lean;
        let mid = base + Vec3::Y * height * 0.6 + lean * 0.2;
        // Stem as a thin two-sided strip.
        let side = lean.cross(Vec3::Y).normalize_or(Vec3::X);
        let (mut l, mut r, mut nrm, mut v, mut col) = (vec![], vec![], vec![], vec![], vec![]);
        for i in 0..=4 {
            let t = i as f32 / 4.0;
            let p = bezier(base, mid, top, t);
            let w = 0.004;
            l.push(p - side * w);
            r.push(p + side * w);
            nrm.push(Vec3::Y);
            v.push(1.0 - t);
            col.push((stem * (0.5 + 0.5 * t)).extend(1.0));
        }
        b.strip(&l, &r, &nrm, &v, &col);
        if bells {
            // Bluebell: a row of nodding bells along the arching stem.
            for k in 0..rng.range_i32(3, 7) {
                let t = 0.55 + 0.45 * k as f32 / 6.0;
                let at = bezier(base, mid, top, t);
                let c = petal * rng.range(0.85, 1.15);
                bell(&mut b, at - Vec3::Y * 0.012, 0.014, 0.028, c.extend(1.0));
            }
        } else {
            // A flat flower head; each stem picks a color.
            let pick = rng.range(0.0, 1.0);
            let (pc, cc) = if pick < 0.4 {
                (petal, Vec3::new(0.9, 0.6, 0.03))
            } else if pick < 0.75 {
                (accent, Vec3::new(0.02, 0.02, 0.02))
            } else if pick < 0.9 {
                (Vec3::new(0.85, 0.6, 0.02), Vec3::new(0.6, 0.35, 0.02))
            } else {
                (Vec3::new(0.32, 0.12, 0.55), Vec3::new(0.85, 0.75, 0.2))
            };
            let face = (Vec3::Y + lean.normalize_or(Vec3::Y) * 0.4).normalize();
            let size = sh.leaf_size_m * rng.range(0.8, 1.3);
            head(
                &mut b,
                top,
                face,
                size,
                pc.extend(1.0),
                cc.extend(1.0),
                &mut rng,
            );
        }
    }
    PlantMeshes {
        foliage: Some(finish_foliage(b, h * 1.1)),
        height: h * 1.1,
        ..Default::default()
    }
}

/// A flower head: petals around a center, facing `face`.
fn head(
    b: &mut MeshBuilder,
    at: Vec3,
    face: Vec3,
    size: f32,
    petal: Vec4,
    center: Vec4,
    rng: &mut Rng,
) {
    let u = face.any_orthonormal_vector();
    let w = face.cross(u);
    let petals = rng.range_i32(5, 9);
    for k in 0..petals {
        let a = k as f32 / petals as f32 * std::f32::consts::TAU;
        let d = u * a.cos() + w * a.sin();
        let s = d.cross(face);
        let tip = at + d * size + face * size * 0.15;
        let corners = [
            at - s * size * 0.12,
            at + s * size * 0.12,
            tip + s * size * 0.18,
            tip - s * size * 0.18,
        ];
        let uv = engine_core::math::Vec2::ZERO;
        b.card(
            corners,
            [uv; 4],
            face,
            [petal * 0.85, petal * 0.85, petal, petal],
        );
    }
    let c = at + face * size * 0.08;
    let ring: Vec<Vec3> = (0..6)
        .map(|k| {
            let a = k as f32 / 6.0 * std::f32::consts::TAU;
            at + (u * a.cos() + w * a.sin()) * size * 0.28 + face * size * 0.02
        })
        .collect();
    let base = b.vertices.len() as u32;
    let uv = engine_core::math::Vec2::ZERO;
    b.vertex(c, face, uv, center);
    for p in &ring {
        b.vertex(*p, face, uv, center * 0.8);
    }
    for k in 0..6u32 {
        let (i, j) = (base + 1 + k, base + 1 + (k + 1) % 6);
        b.tri(base, i, j);
        b.tri(base, j, i);
    }
}

/// A small downward-opening bell.
fn bell(b: &mut MeshBuilder, at: Vec3, radius: f32, length: f32, color: Vec4) {
    let rim: Vec<Vec3> = (0..6)
        .map(|k| {
            let a = k as f32 / 6.0 * std::f32::consts::TAU;
            at + Vec3::new(a.cos() * radius, -length, a.sin() * radius)
        })
        .collect();
    let base = b.vertices.len() as u32;
    let uv = engine_core::math::Vec2::ZERO;
    b.vertex(at, Vec3::Y, uv, color * 0.7);
    for p in &rim {
        let n = (*p - at + Vec3::Y * length).normalize_or(Vec3::Y);
        b.vertex(*p, n, uv, color);
    }
    for k in 0..6u32 {
        let (i, j) = (base + 1 + k, base + 1 + (k + 1) % 6);
        b.tri(base, i, j);
        b.tri(base, j, i);
    }
}

/// A lathed cap profile: `(radius, height)` pairs from the top center outward and back under
/// the cap to the stem.
fn cap_profile(lantern: bool) -> &'static [(f32, f32)] {
    if lantern {
        &[
            (0.0, 1.0),
            (0.28, 0.97),
            (0.55, 0.86),
            (0.8, 0.62),
            (0.96, 0.32),
            (1.0, 0.12),
            (0.9, 0.08),
            (0.55, 0.24),
            (0.2, 0.3),
        ]
    } else {
        &[
            (0.0, 0.58),
            (0.35, 0.54),
            (0.65, 0.43),
            (0.86, 0.26),
            (1.0, 0.06),
            (0.97, 0.0),
            (0.7, -0.03),
            (0.3, -0.04),
        ]
    }
}

fn mushrooms(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let lantern = h > 1.0;
    let mut caps = MeshBuilder::new();
    let mut stems = MeshBuilder::new();
    let n = (sh.branches as i32 + rng.range_i32(-2, 3)).max(1);
    for k in 0..n {
        let base2 = if k == 0 {
            engine_core::math::Vec2::ZERO
        } else {
            rng.in_unit_disk() * sp.crown_radius_m * 0.75
        };
        let base = Vec3::new(base2.x, 0.0, base2.y);
        let height =
            if k == 0 { h } else { h * rng.range(0.3, 0.85) } * if lantern { 0.62 } else { 0.85 };
        let out = base.normalize_or(
            Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)).normalize_or(Vec3::X),
        );
        let lean = out * height * rng.range(0.05, 0.25) * (1.0 + sh.gnarl);
        let top = base + Vec3::Y * height + lean;
        let path: Vec<Vec3> = (0..=4)
            .map(|i| {
                let t = i as f32 / 4.0;
                base + Vec3::Y * height * t + lean * t * t - Vec3::Y * 0.02
            })
            .collect();
        let rs = sp.trunk_radius_m * (height / h).powf(0.6) * rng.range(0.8, 1.2);
        let radius: Vec<f32> = (0..=4)
            .map(|i| rs * (1.25 - 0.35 * i as f32 / 4.0))
            .collect();
        let stem = Vec3::from(sp.bark_color);
        let color = |i: usize, _a: f32| (stem * (0.7 + 0.08 * i as f32)).extend(1.0);
        stems.tube(&path, &radius, 7, 1.0, &color, None, None);

        let axis = (Vec3::Y + lean.normalize_or(Vec3::Y) * 0.35).normalize();
        let cap_r = if lantern {
            sp.crown_radius_m * (height / h).powf(0.7) * rng.range(0.85, 1.05)
        } else {
            rs * rng.range(3.0, 4.6)
        };
        let cap_h = if lantern { cap_r * 1.1 } else { cap_r };
        let rot = Quat::from_rotation_arc(Vec3::Y, axis);
        let profile = cap_profile(lantern);
        let segs = if lantern { 20 } else { 12 };
        let start = caps.vertices.len() as u32;
        let lift = top - axis * cap_h * if lantern { 0.25 } else { 0.0 };
        for (i, &(pr, py)) in profile.iter().enumerate() {
            let prev = profile[i.saturating_sub(1)];
            let next = profile[(i + 1).min(profile.len() - 1)];
            let (dr, dy) = (next.0 - prev.0, next.1 - prev.1);
            let nr = -dy;
            let ny = dr;
            let shade = if i + 3 >= profile.len() {
                1.25
            } else {
                0.75 + 0.25 * pr
            };
            for s in 0..=segs {
                let a = s as f32 / segs as f32 * std::f32::consts::TAU;
                let (sa, ca) = a.sin_cos();
                let p = Vec3::new(ca * pr * cap_r, py * cap_h, sa * pr * cap_r);
                let nrm = Vec3::new(ca * nr, ny, sa * nr).normalize_or(Vec3::Y);
                caps.vertex(
                    lift + rot * p,
                    rot * nrm,
                    engine_core::math::Vec2::new(s as f32 / segs as f32, i as f32),
                    Vec4::new(shade, shade, shade, 1.0),
                );
            }
        }
        let ring = segs + 1;
        for i in 0..profile.len() as u32 - 1 {
            for s in 0..segs {
                let a = start + i * ring + s;
                let quad = [a, a + 1, a + ring + 1, a + ring];
                push_oriented(&mut caps, quad);
            }
        }
    }
    let top = h * 1.25;
    let mut out = PlantMeshes {
        height: top,
        ..Default::default()
    };
    out.wood = Some(stems.finish());
    out.foliage = Some(caps.finish());
    let _ = rgb;
    out
}

/// Add a quad whose winding agrees with its vertex normals.
fn push_oriented(b: &mut MeshBuilder, q: [u32; 4]) {
    let p = |i: u32| Vec3::from(b.vertices[i as usize].position);
    let n = |i: u32| Vec3::from(b.vertices[i as usize].normal);
    let geo =
        (p(q[1]) - p(q[0])).cross(p(q[2]) - p(q[0])) + (p(q[2]) - p(q[0])).cross(p(q[3]) - p(q[0]));
    let avg = n(q[0]) + n(q[1]) + n(q[2]) + n(q[3]);
    if geo.dot(avg) >= 0.0 {
        b.tri(q[0], q[1], q[2]);
        b.tri(q[0], q[2], q[3]);
    } else {
        b.tri(q[0], q[2], q[1]);
        b.tri(q[0], q[3], q[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::PlantLibrary;

    #[test]
    fn small_plants_build() {
        let lib = PlantLibrary::default();
        for name in [
            "fern",
            "grass",
            "wildflowers",
            "bluebell",
            "glowcap",
            "lantern cap",
        ] {
            let sp = lib.get(lib.id(name).unwrap());
            let m = small(sp, 4);
            let f = m.foliage.expect(name);
            assert!(f.triangle_count() > 20, "{name}: {}", f.triangle_count());
            assert!(f.aabb().max.y <= sp.max_height_m * 1.3, "{name}");
            if sp.form == PlantForm::Mushroom {
                assert!(m.wood.is_some(), "{name} stems");
            }
        }
    }
}
