//! A felled tree's crown pressed against the ground under the tree's weight.
//!
//! A tree comes down on its crown: the limbs underneath bend, splay and snap, the trunk settles
//! onto what is left of them, and none of it goes into the ground (playtest 3, #33). The log
//! rests with its axis propped above the ground by part of the crown's radius
//! ([`crate::felling::log_rest_transform`]); here every part below the axis that would reach
//! further down than the ground allows is pressed up toward the axis, more the deeper it hung,
//! and pushed out sideways a little, as crushed limbs splay. The trunk itself keeps its shape.
//! Each mesh a log shows (its levels of detail, its leaves thinning with the seasons) gets its
//! own pressed copy the first time it shows, made for the pose the log came to rest in, and the
//! copies are freed when the log loses its crown or goes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_render::{Mesh, MeshRef, MeshRenderer};
use engine_world::Ground;

use crate::felling::LOG_BARE_YEARS;
use crate::plant::FelledLog;
use crate::visuals::{LogVisual, PlantPart};

/// Grid spacing of the ground samples under a crown, meters.
const GROUND_CELL_M: f64 = 0.5;
/// Pressed limbs stay this far above the ground, meters.
const CLEARANCE_M: f32 = 0.03;
/// How far crushed limbs splay out sideways, per meter they were pressed up.
const SPLAY: f32 = 0.35;

/// A pressed copy being made on the task pool.
type PressSlot = Arc<Mutex<Option<Mesh>>>;

/// On a fallen tree whose crown rests on the ground: the pose it lies in and the pressed copies
/// of the meshes its parts have shown (keyed by the original mesh), and those being made (off
/// the frame: a crown has tens of thousands of vertices and the ground under it is sampled
/// hundreds of times; on the PC one press took 107 ms in the frame).
#[derive(Component, Debug, Clone)]
pub struct CrownSquash {
    /// The log's transform at rest.
    pub rest: Transform,
    /// Trunk radius at the butt, meters (world).
    pub trunk_r: f32,
    /// How far below its axis the crown hangs at most, meters (world).
    pub crown_r: f32,
    pressed: HashMap<u32, Handle<Mesh>>,
    pressing: HashMap<u32, PressSlot>,
}

impl CrownSquash {
    pub fn new(rest: Transform, trunk_r: f32, crown_r: f32) -> Self {
        Self {
            rest,
            trunk_r,
            crown_r,
            pressed: HashMap::new(),
            pressing: HashMap::new(),
        }
    }

    fn is_pressed(&self, h: Handle<Mesh>) -> bool {
        self.pressed.values().any(|p| p.index() == h.index())
    }
}

/// Ground heights on a grid over a footprint (bilinear between samples): a terrain answer costs
/// tens of microseconds, a crown has tens of thousands of vertices.
struct GroundGrid {
    min: DVec2,
    n: [usize; 2],
    h: Vec<f64>,
}

impl GroundGrid {
    fn new(ground: Option<&Ground>, min: DVec2, max: DVec2, hint_y: f64) -> Self {
        let n = [
            ((max.x - min.x) / GROUND_CELL_M).ceil() as usize + 2,
            ((max.y - min.y) / GROUND_CELL_M).ceil() as usize + 2,
        ];
        let mut h = Vec::with_capacity(n[0] * n[1]);
        for j in 0..n[1] {
            for i in 0..n[0] {
                let p = DVec3::new(
                    min.x + i as f64 * GROUND_CELL_M,
                    hint_y + 4.0,
                    min.y + j as f64 * GROUND_CELL_M,
                );
                h.push(ground.and_then(|g| g.height(p)).unwrap_or(0.0));
            }
        }
        Self { min, n, h }
    }

    fn at(&self, x: f64, z: f64) -> f64 {
        let fx = ((x - self.min.x) / GROUND_CELL_M).clamp(0.0, (self.n[0] - 1) as f64);
        let fz = ((z - self.min.y) / GROUND_CELL_M).clamp(0.0, (self.n[1] - 1) as f64);
        let (i, j) = (
            (fx as usize).min(self.n[0] - 2),
            (fz as usize).min(self.n[1] - 2),
        );
        let (tx, tz) = (fx - i as f64, fz - j as f64);
        let g = |i: usize, j: usize| self.h[j * self.n[0] + i];
        let a = g(i, j) + (g(i + 1, j) - g(i, j)) * tx;
        let b = g(i, j + 1) + (g(i + 1, j + 1) - g(i, j + 1)) * tx;
        a + (b - a) * tz
    }
}

/// `mesh` (model space, drawn with `rest`) with everything below its axis that would sink into
/// the ground pressed up against it.
pub fn press_crown(
    mesh: &Mesh,
    rest: &Transform,
    trunk_r: f32,
    crown_r: f32,
    ground: Option<&Ground>,
) -> Mesh {
    let to_world =
        |v: [f32; 3]| rest.translation + (rest.rotation * (rest.scale * Vec3::from(v))).as_dvec3();
    let world: Vec<DVec3> = mesh.vertices.iter().map(|v| to_world(v.position)).collect();
    let (mut min, mut max) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
    for p in &world {
        min = min.min(DVec2::new(p.x, p.z));
        max = max.max(DVec2::new(p.x, p.z));
    }
    let grid = GroundGrid::new(ground, min, max, rest.translation.y);
    let axis = (rest.rotation * Vec3::Y).normalize_or(Vec3::X);
    let origin = rest.translation;
    let inverse_rotation = rest.rotation.inverse();
    let inverse_scale = Vec3::ONE / rest.scale.max(Vec3::splat(1e-4));
    let depth = crown_r.max(0.3);
    let mut out = mesh.clone();
    for (v, p) in out.vertices.iter_mut().zip(&world) {
        let along = (*p - origin).as_vec3().dot(axis);
        let center = origin + (axis * along).as_dvec3();
        let off = (*p - center).as_vec3();
        let below = -off.y;
        // The trunk and the limbs at it keep their shape; only what hangs below the axis sags.
        let keep = smoothstep(1.3 * trunk_r, 2.6 * trunk_r, off.length());
        if below <= 0.0 || keep <= 0.0 {
            continue;
        }
        let floor = grid.at(p.x, p.z) as f32 + CLEARANCE_M;
        // Room between the axis and the ground here, shared out over the crown's depth: the
        // deepest limbs come up the most, so the crown's underside flattens on the ground.
        let room = (center.y as f32 - floor).max(0.0);
        let squeeze = (room / depth).clamp(0.05, 1.0);
        if squeeze >= 1.0 && p.y as f32 >= floor {
            continue;
        }
        let pressed = (below * squeeze).min(room);
        let lift = (below - pressed) * keep;
        // Crushed limbs splay out sideways, away from the axis.
        let side = Vec3::new(off.x, 0.0, off.z) - axis * Vec3::new(off.x, 0.0, off.z).dot(axis);
        let splay = side.normalize_or_zero() * lift * SPLAY;
        let mut q = *p + DVec3::new(splay.x as f64, lift as f64, splay.z as f64);
        q.y = q.y.max(grid.at(q.x, q.z) + CLEARANCE_M as f64);
        let local = inverse_rotation * (q - rest.translation).as_vec3() * inverse_scale;
        v.position = local.into();
    }
    out
}

/// Swap the meshes a resting crown shows for pressed copies, and free them when the log has
/// lost its crown or gone.
#[allow(clippy::type_complexity)]
pub fn press_fallen_crowns(
    mut commands: Commands,
    ground: Option<Res<Ground>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut logs: Query<(Entity, &mut CrownSquash, &LogVisual, Option<&FelledLog>)>,
    mut parts: Query<&mut MeshRenderer, With<PlantPart>>,
    mut gone: RemovedComponents<CrownSquash>,
    mut orphans: Local<Vec<(Entity, Vec<Handle<Mesh>>)>>,
) {
    for (entity, mut squash, visual, log) in &mut logs {
        if log.is_some_and(|l| l.years >= LOG_BARE_YEARS) {
            for (_, h) in squash.pressed.drain() {
                meshes.remove(h);
            }
            squash.pressing.clear();
            commands.entity(entity).remove::<CrownSquash>();
            continue;
        }
        for &part in &visual.parts {
            let Ok(mut r) = parts.get_mut(part) else {
                continue;
            };
            let MeshRef::Handle(h) = r.mesh else { continue };
            if !r.visible || squash.is_pressed(h) {
                continue;
            }
            let key = h.index();
            let pressed = match squash.pressed.get(&key) {
                Some(p) => Some(*p),
                None => match squash.pressing.get(&key) {
                    // Done: it goes in.
                    Some(slot) => match slot.lock().ok().and_then(|mut s| s.take()) {
                        Some(m) => {
                            squash.pressing.remove(&key);
                            let p = meshes.add(m);
                            squash.pressed.insert(key, p);
                            Some(p)
                        }
                        None => None,
                    },
                    None => {
                        let Some(mesh) = meshes.get(h) else { continue };
                        let (rest, trunk_r, crown_r) =
                            (squash.rest, squash.trunk_r, squash.crown_r);
                        match AsyncComputeTaskPool::try_get() {
                            Some(pool) => {
                                let (mesh, ground) = (mesh.clone(), ground.as_deref().cloned());
                                let slot: PressSlot = Arc::new(Mutex::new(None));
                                let out = slot.clone();
                                pool.spawn(async move {
                                    let m = press_crown(
                                        &mesh,
                                        &rest,
                                        trunk_r,
                                        crown_r,
                                        ground.as_ref(),
                                    );
                                    if let Ok(mut s) = out.lock() {
                                        *s = Some(m);
                                    }
                                })
                                .detach();
                                squash.pressing.insert(key, slot);
                                None
                            }
                            None => {
                                let m =
                                    press_crown(mesh, &rest, trunk_r, crown_r, ground.as_deref());
                                let p = meshes.add(m);
                                squash.pressed.insert(key, p);
                                Some(p)
                            }
                        }
                    }
                },
            };
            // Until its copy is made, the part shows as it is (a frame or few after impact).
            if let Some(p) = pressed {
                r.mesh = MeshRef::Handle(p);
            }
        }
        orphans.retain(|(e, _)| *e != entity);
        orphans.push((entity, squash.pressed.values().copied().collect()));
    }
    // Despawned logs: free their copies.
    for e in gone.read() {
        if let Some(i) = orphans.iter().position(|(o, _)| *o == e) {
            for h in orphans.swap_remove(i).1 {
                meshes.remove(h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::mesh::Vertex;

    fn vertex(p: Vec3) -> Vertex {
        Vertex {
            position: p.into(),
            normal: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            color: [1.0; 4],
        }
    }

    #[test]
    fn a_pressed_crown_stays_above_the_ground_and_the_trunk_keeps_its_shape() {
        // A log lying along +X, its axis 1 m above flat ground (model +Y along the trunk).
        let rest = Transform::from_translation(DVec3::new(0.0, 1.0, 0.0))
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, Vec3::X));
        let to_model = |w: Vec3| rest.rotation.inverse() * (w - Vec3::Y);
        let trunk = vertex(to_model(Vec3::new(2.0, 1.0 - 0.2, 0.0)));
        let limb = vertex(to_model(Vec3::new(8.0, 1.0 - 3.0, 1.0)));
        let above = vertex(to_model(Vec3::new(8.0, 1.0 + 2.5, 0.5)));
        let mesh = Mesh::new(vec![trunk, limb, above], vec![0, 1, 2]);
        let out = press_crown(&mesh, &rest, 0.25, 3.0, Some(&Ground::default()));
        let world = |v: &Vertex| rest.transform_point(Vec3::from(v.position).as_dvec3());
        assert_eq!(
            out.vertices[0].position, mesh.vertices[0].position,
            "trunk moved"
        );
        assert_eq!(
            out.vertices[2].position, mesh.vertices[2].position,
            "upper crown moved"
        );
        let pressed = world(&out.vertices[1]);
        assert!(
            pressed.y >= CLEARANCE_M as f64 - 1e-4,
            "limb in the ground: {pressed}"
        );
        assert!(
            pressed.y < 0.6,
            "limb not flattened on the ground: {pressed}"
        );
        assert!(pressed.z > 1.0, "crushed limb splays out: {pressed}");
    }
}
