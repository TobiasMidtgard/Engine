//! Procedural vehicle meshes: smooth lofted bodies (car shells, fuselages, hulls) and
//! airfoil wing panels. Registered under `vehicles/…` keys so every vehicle of a kind shares
//! one GPU mesh.

use engine_assets::Assets;
use engine_core::math::{Vec2, Vec3};
use engine_render::{Mesh, Vertex};

/// One cross-section of a lofted body: a superellipse centered at (0, `y`) in the plane
/// `z`, `half_width` × `half_height`. `power` 2 = ellipse, 4+ = rounded box.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub z: f32,
    pub y: f32,
    pub half_width: f32,
    pub half_height: f32,
    pub power: f32,
}

pub const fn section(z: f32, y: f32, half_width: f32, half_height: f32, power: f32) -> Section {
    Section {
        z,
        y,
        half_width,
        half_height,
        power,
    }
}

fn superellipse(t: f32, a: f32, b: f32, p: f32) -> Vec2 {
    let (s, c) = t.sin_cos();
    let e = 2.0 / p.max(0.5);
    Vec2::new(
        a * c.signum() * c.abs().powf(e),
        b * s.signum() * s.abs().powf(e),
    )
}

/// Connect closed rings of equal size into a closed, outward-facing mesh (with end caps).
pub fn rings_to_mesh(rings: &[Vec<Vec3>], cap_start: bool, cap_end: bool) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let m = rings.first().map_or(0, |r| r.len());
    if rings.len() < 2 || m < 3 {
        return Mesh::default();
    }
    let n = rings.len();
    for (i, ring) in rings.iter().enumerate() {
        for (j, p) in ring.iter().enumerate() {
            vertices.push(Vertex::new(
                *p,
                Vec3::Y,
                Vec2::new(j as f32 / m as f32, i as f32 / (n - 1) as f32),
            ));
        }
    }
    for i in 0..n - 1 {
        for j in 0..m {
            let a = (i * m + j) as u32;
            let b = (i * m + (j + 1) % m) as u32;
            let c = ((i + 1) * m + (j + 1) % m) as u32;
            let d = ((i + 1) * m + j) as u32;
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    let mut add_cap = |ring: &Vec<Vec3>, flip: bool| {
        let center = ring.iter().copied().sum::<Vec3>() / m as f32;
        let base = vertices.len() as u32;
        vertices.push(Vertex::new(center, Vec3::Y, Vec2::splat(0.5)));
        for p in ring {
            vertices.push(Vertex::new(*p, Vec3::Y, Vec2::splat(0.5)));
        }
        for j in 0..m as u32 {
            let (a, b) = (base + 1 + j, base + 1 + (j + 1) % m as u32);
            if flip {
                indices.extend_from_slice(&[base, a, b]);
            } else {
                indices.extend_from_slice(&[base, b, a]);
            }
        }
    };
    if cap_start {
        add_cap(&rings[0], false);
    }
    if cap_end {
        add_cap(&rings[n - 1], true);
    }
    let mut mesh = Mesh::new(vertices, indices);
    orient_outward(&mut mesh);
    mesh.compute_normals();
    mesh
}

/// Flip the winding if the mesh's signed volume says it faces inward.
fn orient_outward(mesh: &mut Mesh) {
    let mut volume = 0.0;
    for t in mesh.indices.as_chunks::<3>().0 {
        let p = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
        volume += p(t[0]).dot(p(t[1]).cross(p(t[2])));
    }
    if volume < 0.0 {
        for t in mesh.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
}

/// A smooth body through `sections` (sorted front to back, along +Z).
pub fn loft(sections: &[Section], ring: usize) -> Mesh {
    let rings: Vec<Vec<Vec3>> = sections
        .iter()
        .map(|s| {
            (0..ring)
                .map(|j| {
                    let t = j as f32 / ring as f32 * std::f32::consts::TAU;
                    let p =
                        superellipse(t, s.half_width.max(1e-3), s.half_height.max(1e-3), s.power);
                    Vec3::new(p.x, s.y + p.y, s.z)
                })
                .collect()
        })
        .collect();
    rings_to_mesh(&rings, true, true)
}

/// NACA 4-digit airfoil outline (unit chord, leading edge at 0, trailing edge at 1),
/// as (x along chord, y) points going around from the trailing edge over the top.
fn airfoil(camber: f32, camber_pos: f32, thickness: f32, points: usize) -> Vec<Vec2> {
    let half = points / 2;
    let mut upper = Vec::with_capacity(half);
    let mut lower = Vec::with_capacity(half);
    for i in 0..half {
        // Cosine spacing: more points near the rounded leading edge.
        let x = 0.5 * (1.0 - (std::f32::consts::PI * i as f32 / (half - 1) as f32).cos());
        let yt = 5.0
            * thickness
            * (0.2969 * x.sqrt() - 0.1260 * x - 0.3516 * x * x + 0.2843 * x.powi(3)
                - 0.1036 * x.powi(4));
        let (yc, slope) = if camber <= 0.0 || camber_pos <= 0.0 {
            (0.0, 0.0)
        } else if x < camber_pos {
            (
                camber / camber_pos.powi(2) * (2.0 * camber_pos * x - x * x),
                2.0 * camber / camber_pos.powi(2) * (camber_pos - x),
            )
        } else {
            (
                camber / (1.0 - camber_pos).powi(2)
                    * (1.0 - 2.0 * camber_pos + 2.0 * camber_pos * x - x * x),
                2.0 * camber / (1.0 - camber_pos).powi(2) * (camber_pos - x),
            )
        };
        let th = slope.atan();
        upper.push(Vec2::new(x - yt * th.sin(), yc + yt * th.cos()));
        lower.push(Vec2::new(x + yt * th.sin(), yc - yt * th.cos()));
    }
    // TE → LE over the top, then LE → TE underneath (skip duplicate LE/TE points).
    let mut out: Vec<Vec2> = upper.iter().rev().copied().collect();
    out.extend(lower.iter().skip(1).take(half - 2));
    out
}

/// A wing panel from x = 0 (root) to x = `span`, chord along −Z with the quarter-chord at
/// z = 0, tip swept back by `sweep` meters and raised by `dihedral` meters.
pub fn wing_panel(
    span: f32,
    root_chord: f32,
    tip_chord: f32,
    sweep: f32,
    dihedral: f32,
    thickness: f32,
    camber: f32,
) -> Mesh {
    let outline = airfoil(camber, 0.4, thickness, 28);
    let stations = 6;
    let rings: Vec<Vec<Vec3>> = (0..=stations)
        .map(|i| {
            let s = i as f32 / stations as f32;
            let chord = root_chord + (tip_chord - root_chord) * s;
            let le = -0.25 * chord + sweep * s;
            outline
                .iter()
                .map(|p| Vec3::new(span * s, dihedral * s + p.y * chord, le + p.x * chord))
                .collect()
        })
        .collect();
    rings_to_mesh(&rings, true, true)
}

/// Mirror a mesh across x = 0 (left wing from right wing), keeping it outward-facing.
pub fn mirror_x(mesh: &Mesh) -> Mesh {
    let mut m = mesh.clone();
    for v in &mut m.vertices {
        v.position[0] = -v.position[0];
        v.normal[0] = -v.normal[0];
    }
    for t in m.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    m
}

/// Mesh keys.
pub const CAR_BODY: &str = "vehicles/car_body";
pub const CAR_CABIN: &str = "vehicles/car_cabin";
pub const RALLY_BODY: &str = "vehicles/rally_body";
pub const RALLY_CABIN: &str = "vehicles/rally_cabin";
pub const PLANE_FUSELAGE: &str = "vehicles/plane_fuselage";
pub const PLANE_CANOPY: &str = "vehicles/plane_canopy";
pub const PLANE_WING_RIGHT: &str = "vehicles/plane_wing_r";
pub const PLANE_WING_LEFT: &str = "vehicles/plane_wing_l";
pub const PLANE_TAIL_RIGHT: &str = "vehicles/plane_tail_r";
pub const PLANE_TAIL_LEFT: &str = "vehicles/plane_tail_l";
pub const PLANE_FIN: &str = "vehicles/plane_fin";
pub const PLANE_CONTROL: &str = "vehicles/plane_control";
pub const SHIP_HULL: &str = "vehicles/ship_hull";
pub const SHIP_CANOPY: &str = "vehicles/ship_canopy";
pub const SHIP_WING_RIGHT: &str = "vehicles/ship_wing_r";
pub const SHIP_WING_LEFT: &str = "vehicles/ship_wing_l";
pub const SHIP_FIN: &str = "vehicles/ship_fin";
pub const SHIP_PLUME: &str = "vehicles/ship_plume";

fn sports_car_body() -> Mesh {
    loft(
        &[
            section(-2.22, -0.10, 0.70, 0.12, 3.0),
            section(-2.15, -0.06, 0.82, 0.20, 3.5),
            section(-1.95, -0.02, 0.86, 0.26, 4.0),
            section(-1.40, 0.02, 0.88, 0.30, 4.5),
            section(-0.60, 0.05, 0.89, 0.33, 4.5),
            section(0.40, 0.07, 0.90, 0.34, 4.5),
            section(1.30, 0.08, 0.90, 0.34, 4.5),
            section(1.95, 0.08, 0.87, 0.32, 4.0),
            section(2.18, 0.08, 0.80, 0.27, 3.5),
            section(2.24, 0.08, 0.70, 0.20, 3.0),
        ],
        40,
    )
}

fn sports_car_cabin() -> Mesh {
    loft(
        &[
            section(-0.75, 0.36, 0.78, 0.02, 3.0),
            section(-0.35, 0.50, 0.74, 0.12, 3.0),
            section(0.10, 0.58, 0.69, 0.18, 3.2),
            section(0.70, 0.58, 0.66, 0.17, 3.2),
            section(1.25, 0.48, 0.66, 0.08, 3.0),
            section(1.65, 0.40, 0.70, 0.02, 3.0),
        ],
        32,
    )
}

fn rally_body() -> Mesh {
    loft(
        &[
            section(-2.05, 0.0, 0.72, 0.16, 3.5),
            section(-1.95, 0.02, 0.84, 0.26, 4.5),
            section(-1.40, 0.05, 0.88, 0.34, 5.0),
            section(-0.40, 0.08, 0.89, 0.36, 5.0),
            section(1.20, 0.10, 0.89, 0.37, 5.0),
            section(1.90, 0.10, 0.86, 0.35, 4.5),
            section(2.05, 0.10, 0.76, 0.28, 3.5),
        ],
        40,
    )
}

fn rally_cabin() -> Mesh {
    loft(
        &[
            section(-0.65, 0.42, 0.80, 0.02, 4.0),
            section(-0.30, 0.62, 0.76, 0.18, 4.0),
            section(0.40, 0.74, 0.74, 0.26, 4.5),
            section(1.30, 0.72, 0.74, 0.24, 4.5),
            section(1.80, 0.50, 0.76, 0.04, 4.0),
        ],
        32,
    )
}

fn plane_fuselage() -> Mesh {
    loft(
        &[
            section(-3.25, 0.00, 0.10, 0.10, 2.0),
            section(-3.10, 0.00, 0.42, 0.42, 2.2),
            section(-2.60, 0.02, 0.55, 0.56, 2.4),
            section(-1.20, 0.05, 0.60, 0.66, 2.6),
            section(0.20, 0.05, 0.58, 0.66, 2.6),
            section(1.60, 0.10, 0.40, 0.48, 2.4),
            section(3.20, 0.22, 0.22, 0.28, 2.2),
            section(4.70, 0.32, 0.08, 0.14, 2.0),
            section(4.85, 0.32, 0.03, 0.06, 2.0),
        ],
        32,
    )
}

fn plane_canopy() -> Mesh {
    loft(
        &[
            section(-1.50, 0.62, 0.30, 0.02, 2.0),
            section(-1.00, 0.78, 0.42, 0.16, 2.0),
            section(-0.20, 0.84, 0.44, 0.22, 2.0),
            section(0.60, 0.76, 0.36, 0.14, 2.0),
            section(1.20, 0.62, 0.20, 0.02, 2.0),
        ],
        24,
    )
}

fn ship_hull() -> Mesh {
    loft(
        &[
            section(-11.2, -0.40, 0.25, 0.20, 2.0),
            section(-10.6, -0.30, 1.10, 0.80, 2.2),
            section(-9.0, -0.15, 1.90, 1.25, 2.5),
            section(-6.0, 0.00, 2.40, 1.55, 3.0),
            section(-1.0, 0.05, 2.60, 1.65, 3.5),
            section(5.0, 0.10, 2.65, 1.70, 3.8),
            section(9.5, 0.15, 2.50, 1.60, 3.5),
            section(10.6, 0.15, 2.30, 1.40, 3.0),
        ],
        40,
    )
}

fn ship_canopy() -> Mesh {
    loft(
        &[
            section(-9.6, 0.70, 0.60, 0.03, 2.0),
            section(-8.8, 1.20, 1.10, 0.35, 2.2),
            section(-7.4, 1.45, 1.20, 0.45, 2.4),
            section(-6.2, 1.40, 1.00, 0.30, 2.2),
            section(-5.4, 1.20, 0.60, 0.03, 2.0),
        ],
        28,
    )
}

/// A cone of exhaust: tip at z = 1, base (nozzle) at z = 0, unit radius at the base.
fn plume() -> Mesh {
    loft(
        &[
            section(0.0, 0.0, 1.0, 1.0, 2.0),
            section(0.15, 0.0, 0.95, 0.95, 2.0),
            section(0.5, 0.0, 0.6, 0.6, 2.0),
            section(1.0, 0.0, 0.02, 0.02, 2.0),
        ],
        20,
    )
}

/// Register every vehicle mesh (idempotent).
pub fn register(meshes: &mut Assets<Mesh>) {
    if meshes.by_key(CAR_BODY).is_some() {
        return;
    }
    meshes.insert_keyed(CAR_BODY, sports_car_body());
    meshes.insert_keyed(CAR_CABIN, sports_car_cabin());
    meshes.insert_keyed(RALLY_BODY, rally_body());
    meshes.insert_keyed(RALLY_CABIN, rally_cabin());
    meshes.insert_keyed(PLANE_FUSELAGE, plane_fuselage());
    meshes.insert_keyed(PLANE_CANOPY, plane_canopy());
    let wing = wing_panel(4.7, 1.6, 1.1, 0.12, 0.30, 0.14, 0.02);
    meshes.insert_keyed(PLANE_WING_LEFT, mirror_x(&wing));
    meshes.insert_keyed(PLANE_WING_RIGHT, wing);
    let tail = wing_panel(1.6, 0.85, 0.6, 0.15, 0.0, 0.10, 0.0);
    meshes.insert_keyed(PLANE_TAIL_LEFT, mirror_x(&tail));
    meshes.insert_keyed(PLANE_TAIL_RIGHT, tail);
    // The fin is a tail panel stood upright: span along +Y.
    let mut fin = wing_panel(1.5, 1.2, 0.7, 0.45, 0.0, 0.10, 0.0);
    fin.transform(&engine_core::math::Mat4::from_rotation_z(
        std::f32::consts::FRAC_PI_2,
    ));
    meshes.insert_keyed(PLANE_FIN, fin);
    // Control surface: a thin slab, 1 m span, hinge at z = 0, trailing edge at z = +1.
    meshes.insert_keyed(
        PLANE_CONTROL,
        wing_panel(1.0, 1.0, 1.0, 0.0, 0.0, 0.06, 0.0),
    );
    meshes.insert_keyed(SHIP_HULL, ship_hull());
    meshes.insert_keyed(SHIP_CANOPY, ship_canopy());
    let delta = wing_panel(5.0, 9.0, 2.0, 5.5, 0.4, 0.06, 0.0);
    meshes.insert_keyed(SHIP_WING_LEFT, mirror_x(&delta));
    meshes.insert_keyed(SHIP_WING_RIGHT, delta);
    let mut fin = wing_panel(3.2, 4.5, 1.6, 2.8, 0.0, 0.08, 0.0);
    fin.transform(&engine_core::math::Mat4::from_rotation_z(
        std::f32::consts::FRAC_PI_2,
    ));
    meshes.insert_keyed(SHIP_FIN, fin);
    meshes.insert_keyed(SHIP_PLUME, plume());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_volume(m: &Mesh) -> f32 {
        m.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
                p(t[0]).dot(p(t[1]).cross(p(t[2]))) / 6.0
            })
            .sum()
    }

    #[test]
    fn bodies_are_closed_and_face_outward() {
        for (name, mesh) in [
            ("car", sports_car_body()),
            ("cabin", sports_car_cabin()),
            ("fuselage", plane_fuselage()),
            ("hull", ship_hull()),
            ("plume", plume()),
            ("wing", wing_panel(4.0, 1.5, 1.0, 0.1, 0.2, 0.12, 0.02)),
            (
                "left wing",
                mirror_x(&wing_panel(4.0, 1.5, 1.0, 0.1, 0.2, 0.12, 0.02)),
            ),
        ] {
            let v = signed_volume(&mesh);
            assert!(v > 0.0, "{name}: volume {v}");
            assert!(mesh.triangle_count() > 50, "{name}");
            for vert in &mesh.vertices {
                assert!(Vec3::from(vert.normal).is_finite(), "{name}");
            }
        }
    }

    #[test]
    fn car_body_has_car_proportions() {
        let aabb = sports_car_body().aabb();
        let size = aabb.max - aabb.min;
        assert!((size.z - 4.46).abs() < 0.1, "{size:?}");
        assert!((size.x - 1.8).abs() < 0.1, "{size:?}");
        let wing = wing_panel(4.7, 1.6, 1.1, 0.12, 0.3, 0.14, 0.02).aabb();
        assert!((wing.max.x - 4.7).abs() < 1e-3);
        // Thickness ≈ 14 % of the root chord.
        assert!((wing.max.y - wing.min.y) > 0.2 && (wing.max.y - wing.min.y) < 0.6);
    }
}
