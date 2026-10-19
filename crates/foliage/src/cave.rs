//! A cave's rock chamber: walls and a vaulted roof sculpted from noise, with stalactites and
//! stalagmites, drawn by [`crate::scenery::SceneryFeature`] like the scenery's rocks
//! (triplanar rock texture, cavity shading, soil at the foot), so the mushroom cave reads as a
//! hollow in the rock rather than a box.

use engine_core::math::noise::fbm3;
use engine_core::math::{Rng, Vec2, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_world::SurfaceMaterials;
use engine_world::physics::{Collider, ColliderShape, RigidBody};
use serde::{Deserialize, Serialize};

use crate::scenery::{RockMesh, RockVertex};

/// A cave's rock chamber seen from inside: walls rising from the floor into a vaulted roof,
/// open at the bottom for a floor (a [`crate::demo::GroundCover`]) and in the −Z wall for its
/// mouth. The origin is the middle of the floor. The rock bulges and falls back into alcoves,
/// breaks into ledges along its strata and drapes in flowstone; stalactites hang from the roof
/// and stalagmites rise along the walls' foot. Moss and lichen grow only in the light near the
/// mouth; deeper in, the rock is bare and wet. The walls cast shadows, so no sun gets in.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct CaveChamber {
    /// Half width (X) and half depth (Z) of the floor inside the walls, meters. The rock
    /// bulges in above the floor here and there, but meets the floor at or beyond this.
    pub half_extents: [f32; 2],
    /// Height of the roof's crown above the floor, meters.
    pub height_m: f32,
    /// Width and height of the mouth in the −Z wall, meters (a width of 0 closes it).
    pub mouth: [f32; 2],
    /// Moss and lichen near the mouth, 0..1.
    pub moss: f32,
    pub lichen: f32,
    /// Albedo multiplier (linear RGB).
    pub tint: [f32; 3],
    pub seed: u64,
    /// Stalactites hanging from the roof (about), and stalagmites along the walls.
    pub stalactites: u32,
    pub stalagmites: u32,
}

impl Default for CaveChamber {
    fn default() -> Self {
        Self {
            half_extents: [15.0, 17.0],
            height_m: 9.0,
            mouth: [10.0, 6.5],
            moss: 0.35,
            lichen: 0.3,
            tint: [1.0, 0.95, 0.86],
            seed: 0xca7e,
            stalactites: 140,
            stalagmites: 26,
        }
    }
}

/// `data[3]` of the stalactites' and stalagmites' vertices (the walls' is 255), and of their
/// tips.
const FORMATION: u8 = 0;
const FORMATION_TIP: u8 = 1;

/// Spacing of the chamber's mesh, meters.
const STEP_M: f32 = 0.32;
/// How far the walls reach below the floor (so no gap shows where they meet it), meters.
const BELOW_FLOOR_M: f32 = 0.6;

/// The chamber's surface before it is sculpted: where the floor's outline is and which way it
/// faces, along its perimeter.
struct Outline {
    points: Vec<Vec2>,
    normals: Vec<Vec2>,
}

impl Outline {
    /// A rounded rectangle (a superellipse) with half extents `h`, sampled every `step` meters.
    fn new(h: Vec2, step: f32) -> Self {
        const N: usize = 4096;
        const POWER: f32 = 4.5;
        let at = |t: f32| {
            let (s, c) = t.sin_cos();
            Vec2::new(
                h.x * c.signum() * c.abs().powf(2.0 / POWER),
                h.y * s.signum() * s.abs().powf(2.0 / POWER),
            )
        };
        let fine: Vec<Vec2> = (0..=N)
            .map(|i| at(i as f32 / N as f32 * std::f32::consts::TAU))
            .collect();
        let mut length = vec![0.0f32; fine.len()];
        for i in 1..fine.len() {
            length[i] = length[i - 1] + fine[i].distance(fine[i - 1]);
        }
        let total = length[N];
        let n = ((total / step).round() as usize).max(16);
        let (mut points, mut normals) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let mut j = 0;
        for k in 0..n {
            let want = total * k as f32 / n as f32;
            while j + 1 < N && length[j + 1] < want {
                j += 1;
            }
            // Between the fine samples: near the axes one is far longer than a step.
            let span = (length[j + 1] - length[j]).max(1e-6);
            let p = fine[j].lerp(fine[j + 1], ((want - length[j]) / span).clamp(0.0, 1.0));
            // The outward normal: the gradient of |x/hx|^p + |z/hz|^p.
            let g = Vec2::new(
                (p.x / h.x).abs().powf(POWER - 1.0) * p.x.signum() / h.x,
                (p.y / h.y).abs().powf(POWER - 1.0) * p.y.signum() / h.y,
            );
            points.push(p);
            normals.push(g.normalize_or(p.normalize_or(Vec2::X)));
        }
        Self { points, normals }
    }
}

/// The vault's profile: (how far out, 1 at the floor's outline .. 0 at the crown; height),
/// from below the floor up to just short of the crown, every `step` meters or so.
fn profile(height: f32, reach: f32, step: f32) -> Vec<(f32, f32)> {
    // Walls rise nearly straight, then arch over into a flattish roof.
    const ARCH: f32 = 3.0;
    let at = |a: f32| {
        let (s, c) = a.sin_cos();
        (
            c.max(0.0).powf(2.0 / ARCH),
            height * s.max(0.0).powf(2.0 / ARCH),
        )
    };
    let mut out = vec![(1.0, -BELOW_FLOOR_M), (1.0, 0.0)];
    let (mut a, mut last) = (0.0f32, at(0.0));
    while a < std::f32::consts::FRAC_PI_2 {
        a += 0.0005;
        let p = at(a);
        let d = ((p.0 - last.0) * reach).hypot(p.1 - last.1);
        if d >= step {
            if p.0 < 0.04 {
                break;
            }
            out.push(p);
            last = p;
        }
    }
    out
}

fn noise(p: Vec3, scale: f32, seed: u64, octaves: u32) -> f32 {
    fbm3(
        (p.x / scale) as f64,
        (p.y / scale) as f64,
        (p.z / scale) as f64,
        seed,
        octaves,
        2.0,
        0.5,
    )
}

/// How the rock is sculpted at a point of the unsculpted vault: how far it is pushed out
/// (positive: back into alcoves; negative: bulging in), how much of that is fine relief in
/// crevices (for cavity shading), and how wet it is.
struct Sculpt {
    out: f32,
    crevice: f32,
    wet: f32,
}

fn sculpt(c: &CaveChamber, p: Vec3, up: f32) -> Sculpt {
    let s = c.seed;
    let h = c.height_m;
    // Bulges and alcoves, and the lumps on them.
    let large = noise(p, 7.0, s ^ 0x11, 3) * 1.5;
    let medium = noise(p, 2.2, s ^ 0x22, 2) * 0.45;
    let small = noise(p, 1.4, s ^ 0x33, 1) * 0.08;
    // Strata: ledges standing out along gently wavy bedding planes.
    let bed = p.y * 0.9 + noise(p, 5.0, s ^ 0x44, 2) * 1.6;
    let ledge = (1.0 - (bed * std::f32::consts::PI).sin().abs()).powi(3) * 0.28;
    // Flowstone: vertical folds draping the walls, wet. A fold spans several grid steps so
    // the walls don't alias into corduroy.
    let wall = 1.0 - smoothstep(0.5, 0.85, up);
    let folds = 1.0
        - fbm3(
            (p.x * 0.8) as f64,
            (p.y * 0.2) as f64,
            (p.z * 0.8) as f64,
            s ^ 0x55,
            1,
            2.0,
            0.5,
        )
        .abs();
    let sheet = smoothstep(0.1, 0.5, noise(p, 4.0, s ^ 0x66, 2)) * wall;
    let flow = folds.powi(2) * 0.3 * sheet;
    let mut out = large + medium + small - ledge * wall - flow;
    // Meet the floor at or behind the outline, bulging in only higher up.
    let foot = smoothstep(0.0, 1.4, p.y);
    out = out.max(-1.2 * foot);
    // The roof's crown stays well above the floor.
    out = out.max(-(h * 0.22) * up);
    Sculpt {
        out,
        crevice: (medium / 0.45 + small / 0.08 * 0.3).clamp(-1.0, 1.0),
        wet: (0.25 + 0.6 * sheet + 0.3 * smoothstep(0.7, 1.0, up)).min(1.0),
    }
}

/// The chamber's mesh, in meters relative to its origin: the sculpted walls and roof (facing
/// in), then the stalactites and stalagmites (facing out).
pub fn cave_chamber_mesh(c: &CaveChamber) -> RockMesh {
    let [hx, hz] = c.half_extents.map(|v| v.max(1.0));
    let h = c.height_m.max(2.0);
    let outline = Outline::new(Vec2::new(hx, hz), STEP_M);
    let rings = profile(h, 0.5 * (hx + hz), STEP_M);
    let cols = outline.points.len();
    let (mouth_w, mouth_h) = (c.mouth[0].max(0.0), c.mouth[1].max(0.0));
    let mouth_at = Vec3::new(0.0, 0.5 * mouth_h, -hz);
    let in_mouth = |p: Vec3, facing_z: f32| {
        if mouth_w <= 0.0 || p.z > -0.5 * hz || facing_z > -0.4 || p.y > mouth_h {
            return false;
        }
        let y = (p.y / mouth_h).max(0.0);
        p.x.abs() < 0.5 * mouth_w * (1.0 - y.powi(4)).max(0.0).sqrt()
    };
    // How much daylight from the mouth reaches a point: moss and lichen need it.
    let daylight = |p: Vec3| {
        if mouth_w <= 0.0 {
            return 0.0;
        }
        1.0 - smoothstep(0.3 * mouth_w, 1.2 * mouth_w, p.distance(mouth_at))
    };

    let mut positions = Vec::with_capacity(cols * rings.len() + 1);
    let mut data = Vec::with_capacity(positions.capacity());
    let mut open = Vec::with_capacity(positions.capacity());
    let mut facing_down = Vec::with_capacity(positions.capacity());
    for (j, &(reach, y)) in rings.iter().enumerate() {
        // The profile's outward normal (out, up) at this ring.
        let (prev, next) = (
            rings[j.saturating_sub(1)],
            rings[(j + 1).min(rings.len() - 1)],
        );
        let t = Vec2::new((next.0 - prev.0) * 0.5 * (hx + hz), next.1 - prev.1);
        let n2 = Vec2::new(t.y, -t.x).normalize_or(Vec2::X);
        let n2 = if n2.x < 0.0 { -n2 } else { n2 };
        for k in 0..cols {
            let f = outline.points[k] * reach;
            let fnormal = outline.normals[k];
            let base = Vec3::new(f.x, y, f.y);
            let outward = Vec3::new(fnormal.x * n2.x, n2.y, fnormal.y * n2.x).normalize();
            let s = sculpt(c, base, n2.y.max(0.0));
            let p = base + outward * s.out;
            positions.push(p);
            open.push(in_mouth(base, outward.z));
            facing_down.push(n2.y);
            // Cavities darken: crevices, and the foot where it meets the floor.
            let cavity = (1.0 - 0.5 * smoothstep(0.0, 0.8, s.crevice))
                * (0.55 + 0.45 * smoothstep(-0.2, 0.5, p.y));
            data.push([
                (cavity.clamp(0.0, 1.0) * 255.0) as u8,
                ((1.0 - daylight(p)) * 255.0) as u8,
                (s.wet * (1.0 - daylight(p)) * 255.0) as u8,
                255,
            ]);
        }
    }
    // The crown.
    let apex = positions.len() as u32;
    let crown = Vec3::new(0.0, h, 0.0);
    let s = sculpt(c, crown, 1.0);
    positions.push(crown + Vec3::Y * s.out);
    open.push(false);
    facing_down.push(1.0);
    data.push([230, 255, (s.wet * 255.0) as u8, 255]);

    let mut indices: Vec<u32> = Vec::new();
    let id = |j: usize, k: usize| (j * cols + k % cols) as u32;
    for j in 0..rings.len() - 1 {
        for k in 0..cols {
            let (a, b, cc, d) = (id(j, k), id(j, k + 1), id(j + 1, k + 1), id(j + 1, k));
            if [a, b, cc, d].iter().all(|&v| open[v as usize]) {
                continue;
            }
            // Wound to face the inside.
            indices.extend_from_slice(&[a, b, cc, a, cc, d]);
        }
    }
    let top = rings.len() - 1;
    for k in 0..cols {
        indices.extend_from_slice(&[id(top, k), id(top, k + 1), apex]);
    }
    let shell_vertices = positions.len();

    // Stalactites: clusters hanging where the roof faces down, away from the daylight.
    let mut rng = Rng::new(c.seed ^ 0x57a1);
    let roof: Vec<usize> = (0..shell_vertices)
        .filter(|&i| {
            facing_down[i] > 0.75 && positions[i].y > 0.55 * h && daylight(positions[i]) < 0.05
        })
        .collect();
    let mut made = 0;
    while made < c.stalactites && !roof.is_empty() {
        let at = positions[roof[rng.range_i32(0, roof.len() as i32) as usize]];
        for _ in 0..rng.range_i32(1, 7) {
            if made >= c.stalactites {
                break;
            }
            let p = at + Vec3::new(rng.range(-0.4, 0.4), 0.0, rng.range(-0.4, 0.4));
            // Most are short; a few reach well down.
            let length = 0.12 + 1.9 * rng.range(0.0, 1.0f32).powi(3);
            let radius = 0.025 + 0.06 * length + rng.range(0.0, 0.03);
            cone(
                &mut positions,
                &mut data,
                &mut indices,
                Cone {
                    base: p + Vec3::Y * 0.5,
                    tip: p - Vec3::Y * length
                        + Vec3::new(rng.range(-0.03, 0.03), 0.0, rng.range(-0.03, 0.03)),
                    radius,
                    profile: Profile::Hanging,
                    seed: rng.next_u64(),
                },
            );
            made += 1;
        }
    }
    // Stalagmites: a short way in from the walls' foot, out of the mouth.
    let mut made = 0;
    let mut tries = 0;
    while made < c.stalagmites && tries < c.stalagmites * 20 {
        tries += 1;
        let k = rng.range_i32(0, cols as i32) as usize;
        let foot = outline.points[k] - outline.normals[k] * rng.range(0.4, 1.3);
        let p = Vec3::new(foot.x, -0.15, foot.y);
        if daylight(p) > 0.3 || p.z < -hz + 2.0 && p.x.abs() < 0.5 * mouth_w + 2.0 {
            continue;
        }
        let height = 0.25 + 1.1 * rng.range(0.0, 1.0f32).powi(2);
        let radius = 0.1 + 0.16 * height;
        for (dx, scale) in [(0.0, 1.0), (rng.range(0.25, 0.45), rng.range(0.4, 0.7))] {
            if scale < 1.0 && rng.chance(0.5) {
                continue;
            }
            let side = Vec3::new(outline.normals[k].y, 0.0, -outline.normals[k].x) * dx;
            cone(
                &mut positions,
                &mut data,
                &mut indices,
                Cone {
                    base: p + side,
                    tip: p + side + Vec3::Y * (height * scale + 0.15),
                    radius: radius * scale,
                    profile: Profile::Standing,
                    seed: rng.next_u64(),
                },
            );
        }
        made += 1;
    }

    // Normals: the shell's face the inside (its winding), the cones' face out.
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let [a, b, cc] = tri.map(|i| i as usize);
        let n = (positions[b] - positions[a]).cross(positions[cc] - positions[a]);
        for v in [a, b, cc] {
            normals[v] += n;
        }
    }
    let vertices = positions
        .iter()
        .zip(&normals)
        .zip(&data)
        .map(|((p, n), d)| RockVertex {
            position: p.to_array(),
            normal: n.normalize_or(Vec3::Y).to_array(),
            data: *d,
        })
        .collect();
    RockMesh { vertices, indices }
}

/// How far out the chamber's shadow caster lies from its rock, meters.
const SHADOW_SHELL_M: f32 = 0.8;

/// The chamber as a shadow caster: its walls and roof pushed [`SHADOW_SHELL_M`] out along
/// their normals (the formations as they are). The rock is one sheet, so seen from the sun its
/// outside and its inside lie at the same depth: wherever a bump turns the inside toward the
/// sun, the shadow map can't tell them apart and the inside lights up in blotches. Pushed out,
/// the sheet that casts lies well in front of every surface inside.
pub fn cave_chamber_shadow_mesh(mesh: &RockMesh) -> RockMesh {
    let vertices = mesh
        .vertices
        .iter()
        .map(|v| {
            let mut v = *v;
            if v.data[3] == 255 {
                let p = Vec3::from_array(v.position) - Vec3::from_array(v.normal) * SHADOW_SHELL_M;
                v.position = p.to_array();
            }
            v
        })
        .collect();
    RockMesh {
        vertices,
        indices: mesh.indices.clone(),
    }
}

/// A chamber's static rock collider: the drawn mesh, walls, roof and formations alike, on an
/// entity of its own that follows the chamber, so feet, wheels and bullets meet the rock where
/// it shows.
#[derive(Component, Debug, Clone, Copy)]
pub struct ChamberCollider {
    pub owner: Entity,
}

/// The collision shape of a chamber: its mesh as a triangle mesh.
pub fn cave_chamber_collider(c: &CaveChamber) -> ColliderShape {
    let mesh = cave_chamber_mesh(c);
    ColliderShape::TriMesh {
        vertices: mesh
            .vertices
            .iter()
            .map(|v| Vec3::from_array(v.position))
            .collect(),
        indices: mesh.indices.as_chunks::<3>().0.to_vec(),
    }
}

/// Give each chamber its collider (rebuilt when the chamber changes), keep it where the
/// chamber is, and remove it with the chamber.
#[allow(clippy::type_complexity)]
pub fn sync_chamber_colliders(
    mut commands: Commands,
    surfaces: Option<Res<SurfaceMaterials>>,
    changed: Query<
        (Entity, &CaveChamber, &Transform),
        (Changed<CaveChamber>, Without<ChamberCollider>),
    >,
    chambers: Query<&Transform, (With<CaveChamber>, Without<ChamberCollider>)>,
    mut colliders: Query<(Entity, &ChamberCollider, &mut Transform)>,
) {
    for (e, collider, mut t) in &mut colliders {
        match chambers.get(collider.owner) {
            Ok(owner) if !changed.contains(collider.owner) => {
                if *t != *owner {
                    *t = *owner;
                }
            }
            _ => commands.entity(e).despawn(),
        }
    }
    let rock = surfaces.map_or_else(Default::default, |s| s.id_or_default("rock"));
    for (e, chamber, t) in &changed {
        commands.spawn((
            *t,
            RigidBody::fixed(),
            Collider {
                shape: cave_chamber_collider(chamber),
                ..Default::default()
            }
            .with_surface(rock)
            .with_density(2600.0),
            ChamberCollider { owner: e },
        ));
    }
}

/// How a cone narrows from its base to its tip.
#[derive(Clone, Copy)]
enum Profile {
    /// A stalactite: a long taper to a fine point.
    Hanging,
    /// A stalagmite: a stout, rounded mound.
    Standing,
}

struct Cone {
    base: Vec3,
    tip: Vec3,
    radius: f32,
    profile: Profile,
    seed: u64,
}

/// A rough cone of rock from `base` to `tip`, appended to the mesh (wound to face out).
fn cone(positions: &mut Vec<Vec3>, data: &mut Vec<[u8; 4]>, indices: &mut Vec<u32>, c: Cone) {
    const SIDES: usize = 7;
    const RINGS: usize = 7;
    let axis = c.tip - c.base;
    let length = axis.length();
    let dir = axis / length.max(1e-4);
    let (e1, e2) = dir.any_orthonormal_pair();
    let first = positions.len() as u32;
    let mut rng = Rng::new(c.seed);
    let lumps: Vec<f32> = (0..SIDES * RINGS).map(|_| rng.range(0.85, 1.15)).collect();
    for r in 0..RINGS {
        let t = r as f32 / RINGS as f32;
        let w = match c.profile {
            Profile::Hanging => (1.0 - t).powf(1.4) * 0.92 + 0.08 * (1.0 - t),
            Profile::Standing => (1.0 - t.powf(1.8)).max(0.0).powf(0.55),
        };
        // Rings of drips: little bulges down a stalactite.
        let ring = 1.0 + 0.08 * (t * 19.0 + c.seed as f32 * 1e-9).sin();
        let center = c.base + axis * t;
        for s in 0..SIDES {
            let a = s as f32 / SIDES as f32 * std::f32::consts::TAU;
            let out = e1 * a.cos() + e2 * a.sin();
            positions.push(center + out * c.radius * w * ring * lumps[r * SIDES + s]);
            data.push([200, 255, 200, FORMATION]);
        }
    }
    let tip = positions.len() as u32;
    positions.push(c.tip);
    data.push([200, 255, 230, FORMATION_TIP]);
    let id = |r: usize, s: usize| first + (r * SIDES + s % SIDES) as u32;
    // Wound to face out, whichever way round the sides run.
    let at = |i: u32| positions[i as usize];
    let face = (at(id(0, 1)) - at(id(0, 0))).cross(at(id(1, 1)) - at(id(0, 0)));
    let out = face.dot(at(id(0, 0)) - c.base) > 0.0;
    let mut tri = |a: u32, b: u32, cc: u32| {
        if out {
            indices.extend_from_slice(&[a, b, cc]);
        } else {
            indices.extend_from_slice(&[a, cc, b]);
        }
    };
    for r in 0..RINGS - 1 {
        for s in 0..SIDES {
            let (a, b, cc, d) = (id(r, s), id(r, s + 1), id(r + 1, s + 1), id(r + 1, s));
            tri(a, b, cc);
            tri(a, cc, d);
        }
    }
    for s in 0..SIDES {
        tri(id(RINGS - 1, s), id(RINGS - 1, s + 1), tip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> (CaveChamber, RockMesh) {
        let c = CaveChamber::default();
        let m = cave_chamber_mesh(&c);
        (c, m)
    }

    #[test]
    fn the_chamber_is_a_vault_open_at_its_mouth() {
        let (c, m) = mesh();
        let [hx, hz] = c.half_extents;
        let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
        // The walls meet the floor at or beyond its outline, and the roof stays up.
        let foot = m
            .vertices
            .iter()
            .filter(|v| v.data[3] == 255)
            .map(|v| Vec3::from(v.position))
            .filter(|p| p.y.abs() < 0.05);
        for q in foot {
            let rel = (q.x / hx).abs().powf(4.5) + (q.z / hz).abs().powf(4.5);
            assert!(rel > 0.8, "a wall reaches in at the floor: {q}");
        }
        let top = m
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(0.0f32, f32::max);
        assert!(top > c.height_m * 0.85, "roof at {top}");
        // A ray out through the mouth hits no wall; one through the back wall does.
        let hits = |from: Vec3, dir: Vec3| {
            m.indices.as_chunks::<3>().0.iter().any(|t| {
                let [a, b, cc] = t.map(p);
                ray_hits(from, dir, a, b, cc)
            })
        };
        let middle = Vec3::new(0.0, 2.0, 0.0);
        assert!(!hits(middle, Vec3::NEG_Z));
        assert!(hits(middle, Vec3::Z));
        assert!(hits(middle, Vec3::X));
        assert!(hits(middle, Vec3::Y));
    }

    #[test]
    fn the_collider_is_a_clean_triangle_mesh_that_follows_the_chamber() {
        // Physics backends build half-edge topology from it: no degenerate triangles, each
        // edge in at most two triangles, and those wound the same way round.
        let c = CaveChamber::default();
        let ColliderShape::TriMesh { vertices, indices } = cave_chamber_collider(&c) else {
            panic!("not a triangle mesh");
        };
        let mut edges = std::collections::HashMap::new();
        for t in &indices {
            assert!(
                t[0] != t[1] && t[1] != t[2] && t[0] != t[2],
                "degenerate {t:?}"
            );
            let [a, b, cc] = t.map(|i| vertices[i as usize]);
            assert!((b - a).cross(cc - a).length() > 1e-7, "zero-area {t:?}");
            for (u, v) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((u, v)).or_insert(0) += 1;
            }
        }
        for (&(u, v), &n) in &edges {
            assert_eq!(n, 1, "edge {u}-{v} runs the same way in {n} triangles");
        }

        let mut app = App::new();
        app.add_systems(Update, sync_chamber_colliders);
        let chamber = app
            .world
            .spawn((c, Transform::from_xyz(10.0, 2.0, -4.0)))
            .id();
        app.update();
        let mut q = app
            .world
            .query::<(&ChamberCollider, &Transform, &Collider)>();
        let found: Vec<_> = q.iter(&app.world).map(|(o, t, _)| (o.owner, *t)).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, chamber);
        assert_eq!(found[0].1.translation, DVec3::new(10.0, 2.0, -4.0));
        // It moves with the chamber, is rebuilt when the chamber changes and goes with it.
        app.world
            .get_mut::<Transform>(chamber)
            .unwrap()
            .translation
            .x = 12.0;
        app.update();
        let t = q.iter(&app.world).next().unwrap().1.translation;
        assert_eq!(t.x, 12.0);
        app.world.get_mut::<CaveChamber>(chamber).unwrap().height_m = 7.0;
        app.update();
        app.update();
        assert_eq!(q.iter(&app.world).count(), 1);
        app.world.despawn(chamber);
        app.update();
        app.update();
        assert_eq!(q.iter(&app.world).count(), 0);
    }

    #[test]
    fn the_rock_faces_in_and_the_formations_hang_and_stand() {
        let (c, m) = mesh();
        let center = Vec3::new(0.0, 0.4 * c.height_m, 0.0);
        let shell = m
            .vertices
            .iter()
            .filter(|v| v.data[3] == 255 && v.position[1] > 0.5);
        let (inward, total) = shell.fold((0, 0), |(i, n), v| {
            let to = center - Vec3::from(v.position);
            (i + (Vec3::from(v.normal).dot(to) > 0.0) as u32, n + 1)
        });
        assert!(inward as f32 > 0.97 * total as f32, "{inward} of {total}");
        // Stalactite tips hang below the roof; stalagmites rise from the floor.
        let cones: Vec<Vec3> = m
            .vertices
            .iter()
            .filter(|v| v.data[3] == FORMATION_TIP)
            .map(|v| Vec3::from(v.position))
            .collect();
        let hanging = cones.iter().filter(|p| p.y > 0.3 * c.height_m).count();
        let standing = cones.iter().filter(|p| p.y < 2.0).count();
        assert!(hanging as u32 >= c.stalactites * 9 / 10, "{hanging}");
        // Their sides face out: away from the line down the middle to the nearest tip.
        let sides = m.vertices.iter().filter(|v| v.data[3] == FORMATION);
        let (out, total) = sides.fold((0, 0), |(o, n), v| {
            let p = Vec3::from(v.position);
            let tip = cones
                .iter()
                .min_by(|a, b| {
                    a.with_y(0.0)
                        .distance(p.with_y(0.0))
                        .total_cmp(&b.with_y(0.0).distance(p.with_y(0.0)))
                })
                .unwrap();
            let away = (p - *tip).with_y(0.0);
            (
                o + (Vec3::from(v.normal).with_y(0.0).dot(away) > 0.0) as u32,
                n + 1,
            )
        });
        assert!(out as f32 > 0.9 * total as f32, "{out} of {total}");
        assert!(standing >= c.stalagmites as usize, "{standing}");
    }

    #[test]
    fn the_shadow_caster_lies_outside_the_rock() {
        let (c, m) = mesh();
        let shadow = cave_chamber_shadow_mesh(&m);
        assert_eq!(shadow.indices, m.indices);
        let center = Vec3::new(0.0, 0.4 * c.height_m, 0.0);
        let (mut out, mut shell) = (0, 0);
        for (a, b) in m.vertices.iter().zip(&shadow.vertices) {
            let rock = a.data[3] == 255;
            let (a, b) = (Vec3::from(a.position), Vec3::from(b.position));
            if !rock {
                assert_eq!(a, b, "formations cast as they are");
                continue;
            }
            shell += 1;
            assert!((a.distance(b) - SHADOW_SHELL_M).abs() < 1e-3);
            out += (b.distance(center) > a.distance(center)) as u32;
        }
        assert!(out as f32 > 0.97 * shell as f32, "{out} of {shell}");
    }

    #[test]
    fn moss_grows_only_in_the_mouths_daylight() {
        let (c, m) = mesh();
        let [_, hz] = c.half_extents;
        let dark = |near_mouth: bool| {
            let v: Vec<f32> = m
                .vertices
                .iter()
                .filter(|v| v.data[3] == 255 && v.position[1] < 5.0)
                .filter(|v| {
                    if near_mouth {
                        v.position[2] < -hz + 3.0 && v.position[0].abs() < 8.0
                    } else {
                        v.position[2] > 0.0
                    }
                })
                .map(|v| v.data[1] as f32 / 255.0)
                .collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        assert!(dark(true) < 0.7);
        assert!(dark(false) > 0.95);
    }

    /// Möller–Trumbore, either side.
    fn ray_hits(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> bool {
        let (e1, e2) = (b - a, c - a);
        let p = d.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-8 {
            return false;
        }
        let t0 = o - a;
        let u = t0.dot(p) / det;
        let q = t0.cross(e1);
        let v = d.dot(q) / det;
        u >= 0.0 && v >= 0.0 && u + v <= 1.0 && e2.dot(q) / det > 0.0
    }
}
