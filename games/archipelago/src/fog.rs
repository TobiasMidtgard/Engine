//! Fog of war: the whole planet lies under a bank of fog until it has been seen. Buildings
//! and the player's ships and aircraft clear it around them, for good.
//!
//! What has been seen is a mask over the globe (a cube of six square grids, a cell about
//! 220 m). The fog is drawn as a billowing white surface a few hundred meters up that sinks
//! into the ground and the sea where the seen world begins, built in pieces that are rebuilt
//! only where the mask changed.

use std::collections::{HashMap, HashSet};

use engine::core::math::noise::fbm2;
use engine::prelude::*;
use engine::render::{Mesh, NoMeshLod, NoVirtualGeometry, Vertex};

use crate::fleet::Vessel;
use crate::globe::{self, Frame};
use crate::grid;
use crate::island;
use crate::sim::Building;

/// Cells along one face of the mask.
const N: usize = 288;
/// Pieces of fog along one face, and cells along a piece.
const PIECES: usize = 12;
const PIECE: usize = N / PIECES;
/// Where the bank's foot lies, meters above the sea (a little under it, so the edge meets the
/// water and the ground cleanly). Seen ground has no fog surface at all.
const FOOT_M: f64 = -25.0;
/// How steeply the bank rises from its foot (its slope there, as a share of a straight ramp's).
const SHOULDER: f64 = 2.2;

/// One piece of the fog: face, and its place on the face.
type PieceId = (u8, u8, u8);

/// What the player has seen of the planet, and the fog over the rest.
#[derive(Resource)]
pub struct FogOfWar {
    /// 0 unseen .. 255 seen, six faces of `N`×`N`.
    seen: Vec<u8>,
    dirty: HashSet<PieceId>,
    pieces: HashMap<PieceId, (Entity, Handle<Mesh>)>,
    material: Option<Handle<StandardMaterial>>,
    /// Whether the fog is drawn (the tour's views of far lands turn it off).
    pub shown: bool,
    was_shown: bool,
    clock: f32,
}

impl Default for FogOfWar {
    fn default() -> Self {
        Self {
            seen: vec![0; 6 * N * N],
            dirty: all_pieces().collect(),
            pieces: HashMap::new(),
            material: None,
            shown: true,
            was_shown: true,
            clock: 0.0,
        }
    }
}

fn all_pieces() -> impl Iterator<Item = PieceId> {
    (0..6u8).flat_map(|f| {
        (0..PIECES as u8).flat_map(move |j| (0..PIECES as u8).map(move |i| (f, i, j)))
    })
}

/// The face a direction lies on and its place there, each coordinate −1..1.
fn face_uv(d: DVec3) -> (usize, f64, f64) {
    let a = d.abs();
    if a.x >= a.y && a.x >= a.z {
        (if d.x > 0.0 { 0 } else { 1 }, d.y / a.x, d.z / a.x)
    } else if a.y >= a.z {
        (if d.y > 0.0 { 2 } else { 3 }, d.z / a.y, d.x / a.y)
    } else {
        (if d.z > 0.0 { 4 } else { 5 }, d.x / a.z, d.y / a.z)
    }
}

/// The direction of a place on a face (the place may lie past the face's edge).
fn dir_of(face: usize, u: f64, v: f64) -> DVec3 {
    match face {
        0 => DVec3::new(1.0, u, v),
        1 => DVec3::new(-1.0, u, v),
        2 => DVec3::new(v, 1.0, u),
        3 => DVec3::new(v, -1.0, u),
        4 => DVec3::new(u, v, 1.0),
        _ => DVec3::new(u, v, -1.0),
    }
    .normalize()
}

/// The face coordinate of a cell corner.
fn coord(i: f64) -> f64 {
    i / N as f64 * 2.0 - 1.0
}

impl FogOfWar {
    fn cell(&self, face: usize, i: i64, j: i64) -> f64 {
        let n = N as i64;
        if i >= 0 && j >= 0 && i < n && j < n {
            return self.seen[face * N * N + j as usize * N + i as usize] as f64 / 255.0;
        }
        // Past the edge: the cell of the face next door.
        let d = dir_of(face, coord(i as f64 + 0.5), coord(j as f64 + 0.5));
        let (f, u, v) = face_uv(d);
        let at = |c: f64| (((c + 1.0) * 0.5 * N as f64).floor() as i64).clamp(0, n - 1) as usize;
        self.seen[f * N * N + at(v) * N + at(u)] as f64 / 255.0
    }

    /// How much of the ground under planet direction `d` has been seen, 0..1 (smooth).
    pub fn seen_at(&self, d: DVec3) -> f64 {
        let (f, u, v) = face_uv(d);
        let (x, y) = (
            (u + 1.0) * 0.5 * N as f64 - 0.5,
            (v + 1.0) * 0.5 * N as f64 - 0.5,
        );
        let (i, j) = (x.floor(), y.floor());
        let (fx, fy) = (x - i, y - j);
        let (i, j) = (i as i64, j as i64);
        let a = self.cell(f, i, j) * (1.0 - fx) + self.cell(f, i + 1, j) * fx;
        let b = self.cell(f, i, j + 1) * (1.0 - fx) + self.cell(f, i + 1, j + 1) * fx;
        a * (1.0 - fy) + b * fy
    }

    /// How much of the ground round cell corner `(i, j)` of a face is still unseen, 0..1: the
    /// mask blurred over four cells, so the bank's edge is a long slope and not a cliff.
    fn unseen_round(&self, face: usize, i: i64, j: i64) -> f64 {
        let mut sum = 0.0;
        for dj in -2..2 {
            for di in -2..2 {
                sum += self.cell(face, i + di, j + dj);
            }
        }
        1.0 - sum / 16.0
    }

    /// Whether map point `(x, z)` has been seen.
    pub fn is_seen(&self, x: f64, z: f64) -> bool {
        self.seen_at(globe::dir(x, z)) > 0.5
    }

    /// Clear the fog within `radius_m` of planet direction `d` (soft at the rim).
    pub fn reveal(&mut self, d: DVec3, radius_m: f64) {
        let angle = radius_m / globe::RADIUS_M;
        let cos_out = angle.cos();
        for face in 0..6 {
            let axis = dir_of(face, 0.0, 0.0);
            let along = d.dot(axis);
            if along < 0.2 {
                continue;
            }
            // Where the middle falls on this face's plane, and a box round the circle there.
            let (u, v) = match face {
                0 | 1 => (d.y / along, d.z / along),
                2 | 3 => (d.z / along, d.x / along),
                _ => (d.x / along, d.y / along),
            };
            let reach = angle * (1.0 + u * u + v * v) * 1.6 + 2.0 / N as f64;
            let at = |c: f64| ((c + 1.0) * 0.5 * N as f64).floor() as i64;
            let (i0, i1) = (at(u - reach).max(0), at(u + reach).min(N as i64 - 1));
            let (j0, j1) = (at(v - reach).max(0), at(v + reach).min(N as i64 - 1));
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let c = dir_of(face, coord(i as f64 + 0.5), coord(j as f64 + 0.5));
                    let cos = c.dot(d);
                    if cos < cos_out {
                        continue;
                    }
                    let t = cos.clamp(-1.0, 1.0).acos() / angle;
                    let k = 1.0 - ((t - 0.55) / 0.45).clamp(0.0, 1.0);
                    let value = (k * k * (3.0 - 2.0 * k) * 255.0) as u8;
                    let slot = &mut self.seen[face * N * N + j as usize * N + i as usize];
                    if value > *slot {
                        *slot = value;
                        // The piece, and its neighbours when the cell is on its rim.
                        // (The bank's shape reads the mask three cells round a corner.)
                        for (di, dj) in [
                            (0, 0),
                            (-3, 0),
                            (3, 0),
                            (0, -3),
                            (0, 3),
                            (-3, -3),
                            (3, -3),
                            (-3, 3),
                            (3, 3),
                        ] {
                            let (pi, pj) = (
                                (i + di).div_euclid(PIECE as i64),
                                (j + dj).div_euclid(PIECE as i64),
                            );
                            if pi >= 0 && pj >= 0 && pi < PIECES as i64 && pj < PIECES as i64 {
                                self.dirty.insert((face as u8, pi as u8, pj as u8));
                            } else {
                                // Over the face's edge: whichever piece holds that spot.
                                let n = dir_of(
                                    face,
                                    coord((i + di) as f64 + 0.5),
                                    coord((j + dj) as f64 + 0.5),
                                );
                                self.dirty.insert(piece_at(n));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Clear the fog round map point `p`.
    pub fn reveal_map(&mut self, x: f64, z: f64, radius_m: f64) {
        self.reveal(globe::dir(x, z), radius_m);
    }

    /// Share of the planet seen, 0..1.
    pub fn share_seen(&self) -> f64 {
        self.seen.iter().map(|&v| v as f64).sum::<f64>() / (255.0 * self.seen.len() as f64)
    }
}

fn piece_at(d: DVec3) -> PieceId {
    let (f, u, v) = face_uv(d);
    let at = |c: f64| {
        (((c + 1.0) * 0.5 * PIECES as f64).floor() as i64).clamp(0, PIECES as i64 - 1) as u8
    };
    (f as u8, at(u), at(v))
}

/// The fog's top at planet direction `d`, meters above the sea: big slow swells and smaller
/// billows, lifted over high ground so hills don't stand out of it.
fn top(d: DVec3) -> f64 {
    let p = d * globe::RADIUS_M;
    // Noise over the sphere without a seam: three flat fields blended by direction.
    let field = |s: f64, seed: u64, oct: u32| {
        let w = d * d;
        (fbm2(p.y / s, p.z / s, seed, oct, 2.0, 0.5) as f64) * w.x
            + (fbm2(p.z / s, p.x / s, seed + 1, oct, 2.0, 0.5) as f64) * w.y
            + (fbm2(p.x / s, p.y / s, seed + 2, oct, 2.0, 0.5) as f64) * w.z
    };
    let swell = field(4_200.0, 71, 3);
    let billow = field(1_300.0, 81, 3);
    let ground = island::ground(d).max(0.0);
    360.0 + 150.0 * swell + 190.0 * billow.abs() + (ground - 150.0).max(0.0) * 0.9
}

/// The mesh of one piece in its own space (origin at its middle on the sea, +Y up there), or
/// `None` where all of it has been seen. Also returns the planet pose of that space.
fn piece_mesh(fog: &FogOfWar, id: PieceId) -> (Option<Mesh>, DQuat, DVec3) {
    let (face, pi, pj) = (id.0 as usize, id.1 as usize, id.2 as usize);
    let mid = dir_of(
        face,
        coord((pi * PIECE) as f64 + PIECE as f64 * 0.5),
        coord((pj * PIECE) as f64 + PIECE as f64 * 0.5),
    );
    let side = mid.cross(dir_of(face, 0.3, 0.7)).normalize();
    let rot = DQuat::from_mat3(&DMat3::from_cols(side, mid, side.cross(mid))).normalize();
    let origin = mid * globe::RADIUS_M;
    let inv = rot.inverse();
    // Corners one past the piece on every side, for normals that agree across pieces.
    let w = PIECE + 3;
    let mut pos = vec![Vec3::ZERO; w * w];
    let mut shade = vec![0.0f32; w * w];
    let mut rises = vec![0.0f32; w * w];
    let mut risen = vec![false; w * w];
    let mut any = false;
    for j in 0..w {
        for i in 0..w {
            let d = dir_of(
                face,
                coord((pi * PIECE + i) as f64 - 1.0),
                coord((pj * PIECE + j) as f64 - 1.0),
            );
            let unseen = fog.unseen_round(
                face,
                (pi * PIECE + i) as i64 - 1,
                (pj * PIECE + j) as i64 - 1,
            );
            // A rounded shoulder: steep at the foot, flattening toward the top. Under the
            // foot the slope runs straight on down into the ground: the bank's edge on the
            // sea is then a level line of the smooth mask (a foot flattened at the edge put
            // a saw's teeth in it, a cell long).
            let t = (unseen - 0.22) / 0.78;
            let rise = 1.0 - (1.0 - t.clamp(0.0, 1.0)).powf(SHOULDER);
            let h_top = top(d);
            let h = FOOT_M + (h_top - FOOT_M) * if t < 0.0 { SHOULDER * t } else { rise };
            any |= t > 0.0;
            risen[j * w + i] = t > 0.0;
            rises[j * w + i] = rise as f32;
            pos[j * w + i] = (inv * (d * (globe::RADIUS_M + h) - origin)).as_vec3();
            // Hollows a little greyer than crests.
            shade[j * w + i] = ((h_top - 300.0) / 330.0).clamp(0.0, 1.0) as f32;
        }
    }
    if !any {
        return (None, rot, origin);
    }
    let n = PIECE + 1;
    let mut vertices = Vec::with_capacity(n * n);
    for j in 1..=n {
        for i in 1..=n {
            let at = |i: usize, j: usize| pos[j * w + i];
            let dx = at(i + 1, j) - at(i - 1, j);
            let dz = at(i, j + 1) - at(i, j - 1);
            let mut normal = dz.cross(dx).normalize_or_zero();
            if normal.y < 0.0 {
                normal = -normal;
            }
            let p = at(i, j);
            // White crests over blue-grey hollows, and the bank's face going the colour of
            // sea haze toward its foot, so it thins into the water instead of standing on it.
            let crest = shade[j * w + i];
            let hollow = [0.70, 0.76, 0.86];
            let foot = [0.56, 0.68, 0.80];
            let r = rises[j * w + i];
            let body = (r / 0.7).clamp(0.0, 1.0);
            let body = body * body * (3.0 - 2.0 * body);
            let mut color = [1.0f32; 4];
            for c in 0..3 {
                let top = hollow[c] + (1.0 - hollow[c]) * crest;
                color[c] = foot[c] + (top - foot[c]) * body;
            }
            vertices.push(Vertex {
                position: p.to_array(),
                normal: normal.to_array(),
                uv: [p.x * 0.01, p.z * 0.01],
                color,
            });
        }
    }
    // Which way round is up differs by face: wind each triangle to face away from the planet.
    let mut indices = Vec::with_capacity(PIECE * PIECE * 6);
    let v = |i: usize, j: usize| (j * n + i) as u32;
    let p = |i: usize, j: usize| Vec3::from_array(vertices[j * n + i].position);
    let flip = (p(1, 0) - p(0, 0)).cross(p(0, 1) - p(0, 0)).y < 0.0;
    for j in 0..PIECE {
        for i in 0..PIECE {
            // Skip what lies wholly sunk (seen ground).
            let up = |i: usize, j: usize| risen[(j + 1) * w + i + 1];
            if !(up(i, j) || up(i + 1, j) || up(i, j + 1) || up(i + 1, j + 1)) {
                continue;
            }
            let (a, b, c, d) = (v(i, j), v(i + 1, j), v(i + 1, j + 1), v(i, j + 1));
            if flip {
                indices.extend_from_slice(&[a, c, b, a, d, c]);
            } else {
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    if indices.is_empty() {
        return (None, rot, origin);
    }
    (Some(Mesh::new(vertices, indices)), rot, origin)
}

/// What clears the fog: how far a building of each kind sees, meters.
fn sight(kind: crate::defs::Kind) -> f64 {
    use crate::defs::Kind as K;
    match kind {
        K::Harbour => 1_100.0,
        K::Market | K::Chapel => 800.0,
        _ => 600.0,
    }
}

/// Clear the fog around the player's buildings and craft, and rebuild the pieces that
/// changed (a few a frame).
pub fn update(world: &mut World) {
    let dt = world.resource::<Time>().real_delta_secs();
    let Some(mut fog) = world.remove_resource::<FogOfWar>() else {
        return;
    };
    fog.clock -= dt;
    if fog.clock <= 0.0 {
        fog.clock = 0.4;
        let mut q = world.query::<&Building>();
        for b in q.iter(world) {
            let c = grid::footprint_center(b.kind, b.origin, b.rot);
            fog.reveal_map(c.x, c.y, sight(b.kind));
        }
        let mut q = world.query::<&Vessel>();
        for v in q.iter(world) {
            // The player's own: trade ships and aircraft, their smacks, and craft lying idle.
            if v.owned() {
                let far =
                    if v.craft.flies() { 1_600.0 } else { 1_000.0 } * (1.0 + v.kit.sight as f64);
                fog.reveal_map(v.at.x, v.at.z, far);
            }
        }
    }
    // Show or hide all of it.
    if fog.shown != fog.was_shown {
        fog.was_shown = fog.shown;
        for (e, _) in fog.pieces.values() {
            if let Some(mut r) = world.get_mut::<MeshRenderer>(*e) {
                r.visible = fog.shown;
            }
        }
    }
    if fog.dirty.is_empty() || !fog.shown {
        world.insert_resource(fog);
        return;
    }
    // Rebuild: everything at once the first time and in the tour, else nearest pieces first,
    // a few a frame.
    let frame = *world.resource::<Frame>();
    let all = fog.pieces.is_empty() || world.resource::<EngineConfig>().headless;
    let mut ids: Vec<PieceId> = fog.dirty.iter().copied().collect();
    if !all {
        let here = frame.planet(DVec3::ZERO).normalize();
        let far = |id: &PieceId| {
            let (f, i, j) = (id.0 as usize, id.1 as f64, id.2 as f64);
            let d = dir_of(
                f,
                coord((i + 0.5) * PIECE as f64),
                coord((j + 0.5) * PIECE as f64),
            );
            -d.dot(here)
        };
        ids.sort_by(|a, b| far(a).total_cmp(&far(b)));
        ids.truncate(4);
    }
    for id in &ids {
        fog.dirty.remove(id);
    }
    let built: Vec<(Option<Mesh>, DQuat, DVec3)> =
        engine_ui::demo::art::par_map(ids.len(), |k| piece_mesh(&fog, ids[k]));
    let material = match fog.material {
        Some(m) => m,
        None => {
            let m = world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color: [0.92, 0.94, 0.98, 1.0],
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    // Light wraps round the billows, as it does in cloud.
                    subsurface: 0.7,
                    // Black emission: counts as glowing, which keeps the pieces from being
                    // drawn as far-away stand-in cards (they would show the fog they were
                    // baked with, not the fog as it is now).
                    emissive_strength: 1.0,
                    ..Default::default()
                });
            fog.material = Some(m);
            m
        }
    };
    for (id, (mesh, rot, origin)) in ids.into_iter().zip(built) {
        match (mesh, fog.pieces.get(&id).copied()) {
            (Some(mesh), Some((_, handle))) => {
                world.resource_mut::<Assets<Mesh>>().set(handle, mesh);
            }
            (Some(mesh), None) => {
                let handle = world.resource_mut::<Assets<Mesh>>().add(mesh);
                let mut r = MeshRenderer::new(handle, material);
                r.cast_shadows = false;
                let e = world
                    .spawn((
                        Name::new("Fog of war"),
                        Transform::from_translation(frame.world(origin))
                            .with_rotation((frame.rot * rot).normalize().as_quat()),
                        r,
                        NoVirtualGeometry,
                        NoMeshLod,
                    ))
                    .id();
                fog.pieces.insert(id, (e, handle));
            }
            (None, Some((e, handle))) => {
                world.despawn(e);
                world.resource_mut::<Assets<Mesh>>().remove(handle);
                fog.pieces.remove(&id);
            }
            (None, None) => {}
        }
    }
    world.insert_resource(fog);
}

/// The fog at the start: the home island has been seen.
pub fn start(world: &mut World) {
    let mut fog = FogOfWar::default();
    let home = &island::islands()[0];
    fog.reveal_map(home.center.x, home.center.y, home.radius * 1.5);
    world.insert_resource(fog);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_round_trip_and_reveals_cross_their_edges() {
        for (k, d) in [
            DVec3::new(0.3, 0.9, -0.2),
            DVec3::new(-0.8, 0.1, 0.5),
            DVec3::new(0.577, 0.577, 0.577),
            DVec3::new(0.0, -0.2, -1.0),
        ]
        .into_iter()
        .enumerate()
        {
            let d = d.normalize();
            let (f, u, v) = face_uv(d);
            assert!((dir_of(f, u, v) - d).length() < 1e-9, "direction {k}");
        }
        let mut fog = FogOfWar::default();
        fog.dirty.clear();
        // On a cube corner: three faces take part.
        let corner = DVec3::new(1.0, 1.0, 1.0).normalize();
        assert!(fog.seen_at(corner) < 0.01);
        fog.reveal(corner, 1_500.0);
        assert!(fog.seen_at(corner) > 0.95);
        for off in [DVec3::X, DVec3::Y, DVec3::Z] {
            let near = (corner * globe::RADIUS_M + off * 500.0).normalize();
            assert!(fog.seen_at(near) > 0.9, "{off}");
            let far = (corner * globe::RADIUS_M + off * 4_000.0).normalize();
            assert!(fog.seen_at(far) < 0.01, "{off}");
        }
        let faces: HashSet<u8> = fog.dirty.iter().map(|p| p.0).collect();
        assert_eq!(faces.len(), 3, "{:?}", fog.dirty);
        // The home island's fog lifts with the start, and little else.
        assert!(fog.share_seen() < 0.01);
        assert!(!fog.is_seen(2350.0, -1250.0));
    }

    #[test]
    fn a_piece_is_fog_where_unseen_and_nothing_where_seen() {
        let mut fog = FogOfWar::default();
        let d = globe::dir(0.0, 0.0);
        let id = piece_at(d);
        let (mesh, _, _) = piece_mesh(&fog, id);
        let full = mesh.expect("fog over unseen ground").indices.len();
        assert_eq!(full, PIECE * PIECE * 6);
        fog.reveal(d, 1_200.0);
        let (mesh, _, _) = piece_mesh(&fog, id);
        let some = mesh.expect("fog round the clearing").indices.len();
        assert!(some < full, "{some} of {full}");
        fog.reveal(d, 30_000.0);
        assert!(piece_mesh(&fog, id).0.is_none());
    }
}
