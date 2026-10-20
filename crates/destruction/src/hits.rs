//! Turning world events into damage: bullets, blades, explosions, crashes and digging.
//!
//! [`collect_hits_system`] reads the shared `engine_world` messages (`DamageEvent`,
//! `WorldImpulse`, `ProjectileTrace`, `ContactEvent`, `TerrainEdit`) and queues [`Hit`]s;
//! [`apply_hits_system`] applies them to walls, voxel volumes, structural pieces and
//! fracturable props, and turns what breaks off into debris, dust, smoke and noise.

use engine_core::math::{DVec3, Quat, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{ContactEvent, ContactPhase, MassProperties};
use engine_world::{
    DamageEvent, DamageKind, EditOp, EditShape, Env, NoiseEvent, ProjectileTrace, SmokeEmission,
    SurfaceMaterials, TerrainEdit, WorldImpulse,
};

use crate::assets::DestructionAssets;
use crate::debris::{
    Debris, DebrisBody, DebrisLook, DebrisSpawn, DustRequest, PendingDebris, reduce_points,
};
use crate::fracture::{BreakLook, ConvexPoly};
use crate::material::{DestructibleMaterial, DestructionMaterials, MaterialProps};
use crate::meshing::MeshBuilder;
use crate::structure::{
    Fracturable, Obb, PieceState, StructuralPiece, crate_boards, piece_colors, shatter,
    shatter_along_grain,
};
use crate::sync::{VoxelChunk, WallTile};
use crate::voxel::{VoxelOp, VoxelState, VoxelVolume, box_sdf, capsule_sdf};
use crate::wall::{DestructibleWall, LostCell, WallHit, WallLook, WallState, mesh_cells};
use crate::{DestructionSettings, DestructionStats};

/// Smallest wall chunk (m³) a blast throws that bursts on a hard landing.
const THROWN_SHATTER_M3: f32 = 0.003;
/// Fastest a blast throws wall pieces, m/s. A breaching charge is packed to cut, not to
/// launch, so even a thin partition's slabs land within about a dozen metres of the wall.
const BLAST_THROW_MAX: f32 = 20.0;

/// One piece of damage to apply.
#[derive(Debug, Clone)]
pub enum Hit {
    /// A bullet, blade, fist or crash at `point` moving along `dir`. `energy_j` is the energy
    /// that goes into breaking material. With a `target`, only that entity is hit.
    Impact {
        point: DVec3,
        dir: Vec3,
        energy_j: f32,
        /// Smallest crater radius, meters (bullet caliber, hammer head).
        min_radius: f32,
        /// Blunt/slash: wide shallow craters instead of narrow tunnels.
        melee: bool,
        target: Option<Entity>,
    },
    /// A projectile path: hits the first destructible along it and keeps going through soft
    /// walls with less energy.
    Trace {
        from: DVec3,
        to: DVec3,
        energy_j: f32,
        caliber: f32,
    },
    /// An explosion.
    Blast { center: DVec3, energy_j: f32 },
    /// Cut a rectangle out of a wall (thermite, torch). `min`/`max` are wall-local XY; the
    /// piece is pushed away from the `front` (+Z) side when `front` is true. `melt` turns the
    /// removed material into sparks instead of slabs (a burning cut line).
    Cut {
        wall: Entity,
        min: Vec2,
        max: Vec2,
        front: bool,
        melt: bool,
        /// Velocity (wall-local, m/s; +Z is out of the front) a panel the cut frees leaves
        /// with, in place of the push away from the charge. Zero keeps the push.
        tip: Vec3,
    },
    /// Dig, fill, smooth or paint voxel volumes (from `TerrainEdit`).
    Dig(TerrainEdit),
    /// Fire weakening a piece.
    Burn { target: Entity, energy_j: f32 },
}

/// Hits waiting to be applied (filled from messages and API calls).
#[derive(Resource, Debug, Default)]
pub struct HitQueue {
    pub hits: Vec<Hit>,
}

/// The entity destruction names as `source` on the messages it sends, so it can ignore its
/// own `TerrainEdit`s.
#[derive(Resource, Debug, Clone, Copy)]
pub struct DestructionSource(pub Entity);

/// Turn world messages into [`Hit`]s.
#[allow(clippy::too_many_arguments)]
pub fn collect_hits_system(
    settings: Res<DestructionSettings>,
    source: Option<Res<DestructionSource>>,
    mut queue: ResMut<HitQueue>,
    mut damage: MessageReader<DamageEvent>,
    mut impulses: MessageReader<WorldImpulse>,
    mut traces: MessageReader<ProjectileTrace>,
    mut contacts: MessageReader<ContactEvent>,
    mut edits: MessageReader<TerrainEdit>,
    masses: Query<&MassProperties>,
    destructible: Query<
        (),
        Or<(
            With<WallState>,
            With<WallTile>,
            With<StructuralPiece>,
            With<Fracturable>,
            With<VoxelChunk>,
        )>,
    >,
) {
    let jpd = settings.joules_per_damage;
    for d in damage.read() {
        let energy = d.amount.max(0.0) * jpd;
        let dir = d.impulse.normalize_or_zero();
        let hit = match d.kind {
            DamageKind::Ballistic | DamageKind::Pierce => Hit::Impact {
                point: d.point,
                dir,
                energy_j: energy,
                min_radius: 0.012,
                melee: false,
                target: Some(d.target),
            },
            // Area blasts arrive as `WorldImpulse`; this is the share aimed at this target.
            DamageKind::Explosive => Hit::Impact {
                point: d.point,
                dir,
                energy_j: d.amount.max(0.0) * settings.explosive_joules_per_damage,
                min_radius: 0.25,
                melee: true,
                target: Some(d.target),
            },
            DamageKind::Fire => Hit::Burn {
                target: d.target,
                energy_j: energy,
            },
            _ => Hit::Impact {
                point: d.point,
                dir,
                energy_j: energy,
                min_radius: 0.08,
                melee: true,
                target: Some(d.target),
            },
        };
        queue.hits.push(hit);
    }
    for i in impulses.read() {
        if i.energy_j > 0.0 {
            queue.hits.push(Hit::Blast {
                center: i.center,
                energy_j: i.energy_j,
            });
        }
    }
    for t in traces.read() {
        if !settings.projectile_traces {
            continue;
        }
        let energy =
            0.5 * settings.bullet_mass_kg * t.speed * t.speed * settings.ballistic_efficiency;
        queue.hits.push(Hit::Trace {
            from: t.from,
            to: t.to,
            energy_j: energy,
            caliber: t.radius.clamp(0.004, 0.02),
        });
    }
    for c in contacts.read() {
        if !settings.contact_damage || c.phase != ContactPhase::Started || c.sensor {
            continue;
        }
        if c.impulse < settings.contact_min_impulse {
            continue;
        }
        for (hit, other, dir) in [(c.b, c.a, c.normal), (c.a, c.b, -c.normal)] {
            if destructible.get(hit).is_err() {
                continue;
            }
            let m = masses
                .get(other)
                .map(|m| m.mass_kg)
                .unwrap_or(500.0)
                .max(1.0);
            // Kinetic energy the impulse took out of the other body.
            let energy = c.impulse * c.impulse / (2.0 * m) * 0.5;
            if energy < 200.0 {
                continue;
            }
            queue.hits.push(Hit::Impact {
                point: c.point,
                dir,
                energy_j: energy,
                min_radius: 0.15,
                melee: true,
                target: Some(hit),
            });
        }
    }
    let own = source.map(|s| s.0);
    for e in edits.read() {
        if e.source.is_some() && e.source == own {
            continue;
        }
        queue.hits.push(Hit::Dig(*e));
    }
}

/// Everything destruction can damage.
#[derive(SystemParam)]
pub struct Targets<'w, 's> {
    pub walls: Query<
        'w,
        's,
        (
            Entity,
            &'static GlobalTransform,
            &'static DestructibleWall,
            &'static mut WallState,
        ),
    >,
    pub voxels: Query<
        'w,
        's,
        (
            Entity,
            &'static GlobalTransform,
            &'static VoxelVolume,
            &'static mut VoxelState,
        ),
    >,
    pub pieces: Query<
        'w,
        's,
        (
            Entity,
            &'static GlobalTransform,
            &'static StructuralPiece,
            &'static mut PieceState,
        ),
    >,
    pub fracturables: Query<
        'w,
        's,
        (Entity, &'static GlobalTransform, &'static mut Fracturable),
        Without<StructuralPiece>,
    >,
    pub debris: Query<'w, 's, (&'static Transform, &'static Debris, &'static mut DebrisBody)>,
    pub tiles: Query<'w, 's, &'static WallTile>,
    pub chunks: Query<'w, 's, &'static VoxelChunk>,
}

/// Where destruction's side effects go.
#[derive(SystemParam)]
pub struct Outputs<'w> {
    pub pending: ResMut<'w, PendingDebris>,
    pub stats: ResMut<'w, DestructionStats>,
    pub smoke: MessageWriter<'w, SmokeEmission>,
    pub noise: MessageWriter<'w, NoiseEvent>,
    pub terrain: MessageWriter<'w, TerrainEdit>,
    pub assets: Option<Res<'w, DestructionAssets>>,
}

impl Outputs<'_> {
    /// Surfaces carry textures (see [`DestructionAssets::textured`]).
    pub fn textured(&self) -> bool {
        self.assets.as_ref().is_some_and(|a| a.textured)
    }
}

/// Shared context for applying hits.
pub struct Ctx<'a> {
    pub settings: &'a DestructionSettings,
    pub table: &'a DestructionMaterials,
    /// Surfaces carry textures, so meshes skip their vertex-color patterns.
    pub textured: bool,
    pub rng: &'a mut Rng,
    pub pending: &'a mut PendingDebris,
    pub stats: &'a mut DestructionStats,
    /// Clouds for the smoke module: position, size (m), darkness, temperature (°C).
    pub dust_events: Vec<(DVec3, f32, f32, f32)>,
    pub noise_events: Vec<(DVec3, f32)>,
}

impl Ctx<'_> {
    pub(crate) fn dust(&mut self, at: DVec3, size: f32, darkness: f32, count: u32, velocity: Vec3) {
        self.pending.dust.push(DustRequest {
            position: at,
            velocity,
            size,
            darkness,
            count,
        });
        self.dust_events.push((at, size, darkness, 25.0));
    }

    pub(crate) fn chips(
        &mut self,
        at: DVec3,
        base: Vec3,
        spread: f32,
        count: usize,
        material: DestructibleMaterial,
        size: f32,
    ) {
        for _ in 0..count.min(16) {
            let v = base + self.rng.unit_vector() * spread * self.rng.range(0.3, 1.0);
            let s = size * self.rng.range(0.5, 1.3);
            let variant = self.rng.range_i32(0, 6) as usize;
            let props = self.table.get(material);
            self.pending.items.push(DebrisSpawn {
                position: at + (self.rng.unit_vector() * size).as_dvec3(),
                rotation: Quat::from_euler(
                    engine_core::math::EulerRot::XYZ,
                    self.rng.range(0.0, std::f32::consts::TAU),
                    self.rng.range(0.0, std::f32::consts::TAU),
                    0.0,
                ),
                velocity: v,
                angular: self.rng.unit_vector() * self.rng.range(2.0, 15.0),
                look: DebrisLook::Chip { size: s, variant },
                half_extents: Vec3::splat(s * 0.5),
                hull: Vec::new(),
                mass_kg: s * s * s * props.density * 0.5,
                material,
                shatter_speed: 0.0,
                lifetime: None,
                reuse: None,
                scale: None,
                resting: false,
                from_rock: false,
            });
        }
    }
}

/// Apply every queued hit.
pub fn apply_hits_system(
    mut commands: Commands,
    mut queue: ResMut<HitQueue>,
    settings: Res<DestructionSettings>,
    table: Res<DestructionMaterials>,
    surfaces: Res<SurfaceMaterials>,
    env: Env,
    source: Option<Res<DestructionSource>>,
    mut targets: Targets,
    mut out: Outputs,
    mut rng: Local<Option<Rng>>,
) {
    if queue.hits.is_empty() {
        return;
    }
    let rng = rng.get_or_insert_with(|| Rng::new(0x5EED_DE57));
    let hits = std::mem::take(&mut queue.hits);
    let source = source.map(|s| s.0);
    let mut terrain_edits = Vec::new();
    let mut smoke = Vec::new();
    {
        let textured = out.textured();
        let mut ctx = Ctx {
            settings: &settings,
            table: &table,
            textured,
            rng,
            pending: &mut out.pending,
            stats: &mut out.stats,
            dust_events: Vec::new(),
            noise_events: Vec::new(),
        };
        for hit in hits {
            ctx.stats.hits += 1;
            match hit {
                Hit::Impact {
                    point,
                    dir,
                    energy_j,
                    min_radius,
                    melee,
                    target,
                } => match target {
                    Some(t) => {
                        impact_target(
                            &mut ctx,
                            &mut commands,
                            &mut targets,
                            t,
                            point,
                            dir,
                            energy_j,
                            min_radius,
                            melee,
                        );
                    }
                    None => {
                        let d = dir.normalize_or(Vec3::NEG_Y);
                        let from = point - (d * 0.5).as_dvec3();
                        trace(
                            &mut ctx,
                            &mut commands,
                            &mut targets,
                            from,
                            point + (d * 0.5).as_dvec3(),
                            energy_j,
                            min_radius,
                            melee,
                        );
                    }
                },
                Hit::Trace {
                    from,
                    to,
                    energy_j,
                    caliber,
                } => trace(
                    &mut ctx,
                    &mut commands,
                    &mut targets,
                    from,
                    to,
                    energy_j,
                    caliber,
                    false,
                ),
                Hit::Blast { center, energy_j } => {
                    blast(&mut ctx, &mut commands, &mut targets, center, energy_j);
                    let scale = (energy_j / 1.0e5).cbrt();
                    smoke.push(SmokeEmission {
                        position: center,
                        velocity: Vec3::Y * 3.0,
                        radius: 0.8 * scale,
                        // A smoke grenade's cloud is about 0.5 kg.
                        mass_kg: (0.01 * scale * scale * scale).min(0.5),
                        temperature_c: 250.0,
                        darkness: 0.55,
                    });
                    ctx.noise_events
                        .push((center, (40.0 * scale).clamp(15.0, 600.0)));
                    if settings.terrain_craters
                        && let Some(g) = env.ground(center)
                    {
                        let dirt = table.get(DestructibleMaterial::Dirt);
                        let r = (3.0 * energy_j * 0.3
                            / (2.0 * std::f32::consts::PI * dirt.toughness_j_m3))
                            .cbrt();
                        let above = (center.y - g.height) as f32;
                        if above < r && r > 0.15 {
                            terrain_edits.push(TerrainEdit {
                                center: DVec3::new(
                                    center.x,
                                    g.height + above.max(0.0) as f64 * 0.5,
                                    center.z,
                                ),
                                shape: EditShape::Sphere {
                                    radius: (r - above.max(0.0)).max(0.1),
                                },
                                op: EditOp::Remove,
                                surface: None,
                                source,
                            });
                        }
                    }
                }
                Hit::Cut {
                    wall,
                    min,
                    max,
                    front,
                    melt,
                    tip,
                } => cut(&mut ctx, &mut targets, wall, min, max, front, melt, tip),
                Hit::Dig(edit) => dig(&mut ctx, &mut targets, &edit, &surfaces),
                Hit::Burn { target, energy_j } => {
                    if let Ok((e, gt, piece, mut state)) = targets.pieces.get_mut(target) {
                        let props = table.get(piece.material);
                        if props.fuel_kg_m3 > 0.0 {
                            state.damage_j += energy_j;
                            if state.damage_j >= piece.break_energy(&props) {
                                break_piece(
                                    &mut ctx,
                                    &mut commands,
                                    e,
                                    gt,
                                    piece,
                                    &state,
                                    Vec3::ZERO,
                                    0.0,
                                );
                            }
                        }
                    }
                }
            }
        }
        for (at, size, darkness, temperature_c) in std::mem::take(&mut ctx.dust_events) {
            let hot = temperature_c > 100.0;
            if size > 0.6 || hot {
                // Light: the smoke module keeps clouds around for a long time, and a dense
                // one hides the breach the player is looking at.
                smoke.push(SmokeEmission {
                    position: at,
                    velocity: Vec3::Y * if hot { 2.5 } else { 0.8 },
                    radius: size * 0.6,
                    mass_kg: if hot {
                        0.002
                    } else {
                        (0.008 * size * size).min(0.4)
                    },
                    temperature_c,
                    darkness,
                });
            }
        }
        for (at, radius) in std::mem::take(&mut ctx.noise_events) {
            out.noise.write(NoiseEvent {
                position: at,
                radius,
                source,
            });
        }
    }
    for s in smoke {
        out.smoke.write(s);
    }
    for t in terrain_edits {
        out.terrain.write(t);
    }
}

enum TargetRef {
    Wall(Entity),
    Voxel(Entity),
    Piece(Entity),
    Fract(Entity),
    Debris(Entity),
}

fn resolve(targets: &Targets, e: Entity) -> Option<TargetRef> {
    if targets.walls.contains(e) {
        return Some(TargetRef::Wall(e));
    }
    if let Ok(t) = targets.tiles.get(e) {
        return Some(TargetRef::Wall(t.wall));
    }
    if targets.voxels.contains(e) {
        return Some(TargetRef::Voxel(e));
    }
    if let Ok(c) = targets.chunks.get(e) {
        return Some(TargetRef::Voxel(c.volume));
    }
    if targets.pieces.contains(e) {
        return Some(TargetRef::Piece(e));
    }
    if targets.fracturables.contains(e) {
        return Some(TargetRef::Fract(e));
    }
    if targets.debris.contains(e) {
        return Some(TargetRef::Debris(e));
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn impact_target(
    ctx: &mut Ctx,
    commands: &mut Commands,
    targets: &mut Targets,
    target: Entity,
    point: DVec3,
    dir: Vec3,
    energy: f32,
    min_radius: f32,
    melee: bool,
) -> Option<DVec3> {
    match resolve(targets, target)? {
        TargetRef::Wall(e) => {
            let (_, gt, wall, mut state) = targets.walls.get_mut(e).ok()?;
            let gt = *gt;
            let wall = wall.clone();
            // No direction given: push into the wall from whichever side the point is on.
            let dir = if dir == Vec3::ZERO {
                let local = local_point(&gt, point);
                let n = if local.z >= 0.0 { Vec3::NEG_Z } else { Vec3::Z };
                gt.rotation() * n
            } else {
                dir
            };
            wall_impact(
                ctx, &gt, &wall, &mut state, point, dir, energy, min_radius, melee,
            )
        }
        TargetRef::Voxel(e) => {
            let (_, gt, volume, mut state) = targets.voxels.get_mut(e).ok()?;
            let gt = *gt;
            let dir = if dir == Vec3::ZERO { Vec3::NEG_Y } else { dir };
            voxel_impact(
                ctx, &gt, volume, &mut state, point, dir, energy, min_radius, melee,
            );
            None
        }
        TargetRef::Piece(e) => {
            let (e, gt, piece, mut state) = targets.pieces.get_mut(e).ok()?;
            let gt = *gt;
            let props = ctx.table.get(piece.material);
            state.damage_j += energy;
            state.last_hit = Some(local_point(&gt, point));
            ctx.chips(point, -dir * 2.0, 2.0, 2, piece.material, props.chip_size);
            if state.damage_j >= piece.break_energy(&props) {
                let piece = piece.clone();
                let st = *state;
                break_piece(
                    ctx,
                    commands,
                    e,
                    &gt,
                    &piece,
                    &st,
                    dir,
                    (energy * 0.02).sqrt().min(6.0),
                );
            }
            None
        }
        TargetRef::Fract(e) => {
            let (e, gt, mut f) = targets.fracturables.get_mut(e).ok()?;
            let gt = *gt;
            let props = ctx.table.get(f.material);
            f.damage_j += energy;
            ctx.chips(point, -dir * 2.0, 2.0, 2, f.material, props.chip_size);
            if f.damage_j >= f.break_energy(&props) {
                let f = f.clone();
                let impact = local_point(&gt, point);
                break_fracturable(
                    ctx,
                    commands,
                    e,
                    &gt,
                    &f,
                    impact,
                    dir,
                    (energy * 0.02).sqrt().min(8.0),
                    Vec3::ZERO,
                );
            }
            None
        }
        TargetRef::Debris(e) => {
            if let Ok((_, d, mut body)) = targets.debris.get_mut(e) {
                // Speed change from a share of the energy, as an impulse (kick divides by mass).
                let dv = (2.0 * energy / d.mass_kg.max(0.01)).sqrt().min(15.0) * 0.3;
                body.kick(dir * dv * d.mass_kg, Vec3::ZERO);
            }
            None
        }
    }
}

fn local_point(gt: &GlobalTransform, p: DVec3) -> Vec3 {
    gt.affine().inverse().transform_point3(p).as_vec3()
}

fn local_dir(gt: &GlobalTransform, d: Vec3) -> Vec3 {
    gt.rotation().inverse() * d
}

#[allow(clippy::too_many_arguments)]
fn wall_impact(
    ctx: &mut Ctx,
    gt: &GlobalTransform,
    wall: &DestructibleWall,
    state: &mut WallState,
    point: DVec3,
    dir: Vec3,
    energy: f32,
    min_radius: f32,
    melee: bool,
) -> Option<DVec3> {
    let props = ctx.table.get(wall.material);
    let lp = local_point(gt, point);
    let ld = local_dir(gt, dir).normalize_or(Vec3::NEG_Z);
    let aspect = if melee { 0.35 } else { 1.0 };
    let mut hit = state.crater(&props, lp, ld, energy, min_radius, aspect);
    if let Some(brick) = wall.brick_size() {
        state.knock_out_bricks(brick, &mut hit);
    }
    let exit = hit.exit;
    // Chips spray back toward the shooter from the entry point.
    let t = state.grid.thickness;
    let face_z = if ld.z < 0.0 { t * 0.5 } else { -t * 0.5 };
    let s = if ld.z.abs() > 1e-3 {
        (face_z - lp.z) / ld.z
    } else {
        0.0
    };
    let entry = lp + ld * s;
    if hit.volume > 0.0 {
        let world_entry = gt.transform_point(entry.as_dvec3());
        let back = gt.rotation() * (-ld);
        let count = ((hit.volume / props.chip_size.powi(3)).sqrt() as usize).clamp(1, 6);
        ctx.chips(
            world_entry,
            back * 3.0,
            2.5,
            count,
            wall.material,
            props.chip_size * 0.7,
        );
        ctx.dust(
            world_entry,
            0.25 + hit.volume.cbrt(),
            props.dust_darkness,
            1,
            back * 0.8,
        );
        if let Some(x) = exit {
            let world_exit = gt.transform_point(x.as_dvec3());
            ctx.chips(
                world_exit,
                gt.rotation() * ld * 4.0,
                2.0,
                count / 2 + 1,
                wall.material,
                props.chip_size * 0.6,
            );
            ctx.dust(world_exit, 0.3, props.dust_darkness, 1, gt.rotation() * ld);
        }
        ctx.noise_events
            .push((world_entry, if melee { 20.0 } else { 30.0 }));
    }
    finish_wall_hit(ctx, gt, wall, state, &hit, None);
    exit.map(|x| gt.transform_point(x.as_dvec3()))
}

/// Remesh, spawn chunks for lost cells, and drop unsupported parts.
fn finish_wall_hit(
    ctx: &mut Ctx,
    gt: &GlobalTransform,
    wall: &DestructibleWall,
    state: &mut WallState,
    hit: &WallHit,
    push: Option<(Vec3, f32)>,
) {
    if let Some(rect) = hit.rect {
        state.mark_dirty(rect);
    }
    ctx.stats.cells_destroyed += hit.lost.len() as u64;
    if push.is_some() && !hit.lost.is_empty() {
        let max = ctx.settings.max_chunks_per_hit;
        let per = ctx.settings.cells_per_chunk;
        wall_chunks(ctx, gt, wall, state, &hit.lost, push, max, per, false);
    }
    let islands = state.detach_unsupported();
    for island in islands {
        let per = ctx.settings.cells_per_chunk * 6;
        let max = ctx.settings.max_chunks_per_hit;
        let props = ctx.table.get(wall.material);
        // The dust where a big piece tore away.
        let n = island.len() as f32;
        let c = island
            .iter()
            .map(|c| state.grid.center(c.idx))
            .sum::<Vec2>()
            / n;
        let at = gt.transform_point(DVec3::new(c.x as f64, c.y as f64, 0.0));
        ctx.dust(
            at,
            (n * state.grid.cell.x * state.grid.cell.y)
                .sqrt()
                .clamp(0.3, 3.0),
            props.dust_darkness,
            3,
            Vec3::ZERO,
        );
        wall_chunks(ctx, gt, wall, state, &island, None, max, per, true);
        ctx.stats.collapses += 1;
    }
}

/// Which way (wall-local) a blast at `origin` throws the piece of its wall at `at`: away from
/// the blast and out of the far face, not along the wall, so a charge on a floor rains its
/// hole into the room below and one on a wall blows it into the room behind.
fn thrown(at: Vec3, origin: Vec3) -> Vec3 {
    let away = (at - origin).normalize_or(Vec3::Z);
    let through = if origin.z > 0.0 { Vec3::NEG_Z } else { Vec3::Z };
    (away * 0.5 + through).normalize_or(through)
}

/// Group lost cells into irregular chunks (Voronoi around random seed cells) and spawn them.
#[allow(clippy::too_many_arguments)]
fn wall_chunks(
    ctx: &mut Ctx,
    gt: &GlobalTransform,
    wall: &DestructibleWall,
    state: &WallState,
    cells: &[LostCell],
    push: Option<(Vec3, f32)>,
    max_chunks: usize,
    cells_per_chunk: usize,
    falling: bool,
) {
    if cells.is_empty() {
        return;
    }
    let props = ctx.table.get(wall.material);
    let g = &state.grid;
    let k = (cells.len() / cells_per_chunk.max(1)).clamp(1, max_chunks.max(1));
    let seeds: Vec<Vec2> = (0..k)
        .map(|_| g.center(cells[ctx.rng.range_i32(0, cells.len() as i32) as usize].idx))
        .collect();
    let mut groups: Vec<Vec<LostCell>> = vec![Vec::new(); k];
    for c in cells {
        let p = g.center(c.idx)
            + Vec2::new(ctx.rng.range(-0.5, 0.5), ctx.rng.range(-0.5, 0.5)) * g.cell * 1.5;
        let best = seeds
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.distance_squared(p).total_cmp(&b.1.distance_squared(p)))
            .map(|(i, _)| i)
            .unwrap_or(0);
        groups[best].push(*c);
    }
    let mut look = WallLook::new(wall, ctx.table).with_texture(ctx.textured);
    look.straight_border = false;
    let rot = gt.rotation();
    for group in groups {
        if group.is_empty() {
            continue;
        }
        let area = g.cell.x * g.cell.y;
        let t = g.thickness;
        if group.len() == 1 {
            let c = g.center(group[0].idx);
            let at = gt.transform_point(DVec3::new(c.x as f64, c.y as f64, 0.0));
            let v = push
                .map(|(o, speed)| rot * thrown(Vec3::new(c.x, c.y, 0.0), o) * speed * 0.6)
                .unwrap_or(Vec3::ZERO);
            ctx.chips(at, v, 1.5, 1, wall.material, g.cell.x.max(g.cell.y));
            continue;
        }
        let grid = state.chunk_grid(&group);
        // Center of mass (wall-local).
        let mut m = 0.0;
        let mut com = Vec3::ZERO;
        for idx in 0..grid.len() {
            if !grid.solid[idx] {
                continue;
            }
            let r = grid.remaining(idx);
            let c = grid.center(idx);
            let z = (grid.back[idx] - grid.front[idx]) * 0.5;
            com += Vec3::new(c.x, c.y, z) * r;
            m += r;
        }
        if m <= 0.0 {
            continue;
        }
        com /= m;
        let volume = m * area;
        let mb = mesh_cells(&grid, &look, (0, 0, grid.nx, grid.ny), -com);
        if mb.is_empty() {
            continue;
        }
        let pts: Vec<Vec3> = mb.vertices.iter().map(|v| Vec3::from(v.position)).collect();
        let (lo, hi) = pts.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), p| (lo.min(*p), hi.max(*p)),
        );
        let half = ((hi - lo) * 0.5).max(Vec3::splat(t * 0.25));
        let size = half.max_element() * 2.0;
        let velocity = match push {
            Some((origin, speed)) => {
                let d = thrown(com, origin);
                let falloff = 1.0 / (1.0 + (com - origin).length());
                rot * d * speed * falloff * ctx.rng.range(0.6, 1.25) + ctx.rng.unit_vector() * 0.8
            }
            None => ctx.rng.unit_vector() * 0.3,
        };
        let spin = if push.is_some() { 6.0 } else { 0.6 } / size.max(0.2);
        // Pieces a blast throws burst when they land hard, so a plaster or brick wall doesn't
        // litter the yard with whole slabs of itself.
        let shatter =
            if (falling && volume > 0.03) || (push.is_some() && volume > THROWN_SHATTER_M3) {
                props.shatter_speed
            } else {
                0.0
            };
        ctx.pending.items.push(DebrisSpawn {
            position: gt.transform_point(com.as_dvec3()),
            rotation: rot,
            velocity,
            angular: ctx.rng.unit_vector() * spin.min(15.0),
            look: DebrisLook::Mesh(mb.build()),
            half_extents: half,
            hull: reduce_points(&pts, 24),
            mass_kg: (volume * props.density).max(0.01),
            material: wall.material,
            shatter_speed: shatter,
            lifetime: None,
            reuse: None,
            scale: None,
            resting: false,
            from_rock: false,
        });
    }
}

/// Follow a projectile path through destructibles.
#[allow(clippy::too_many_arguments)]
fn trace(
    ctx: &mut Ctx,
    commands: &mut Commands,
    targets: &mut Targets,
    from: DVec3,
    to: DVec3,
    energy: f32,
    caliber: f32,
    melee: bool,
) {
    let mut origin = from;
    let mut energy = energy;
    for _ in 0..4 {
        let seg = to - origin;
        let len = seg.length() as f32;
        if len < 1e-4 || energy < 1.0 {
            return;
        }
        let dir = (seg / len as f64).as_vec3();
        let Some((t, target)) = first_hit(targets, origin, dir, len) else {
            return;
        };
        let point = origin + (dir * t).as_dvec3();
        let exit = impact_target(
            ctx, commands, targets, target, point, dir, energy, caliber, melee,
        );
        match exit {
            Some(x) if matches!(resolve(targets, target), Some(TargetRef::Wall(_))) => {
                origin = x + (dir * 0.01).as_dvec3();
                energy *= 0.45;
            }
            _ => return,
        }
    }
}

/// Nearest destructible along a ray: (distance, entity).
fn first_hit(targets: &Targets, origin: DVec3, dir: Vec3, max: f32) -> Option<(f32, Entity)> {
    let mut best: Option<(f32, Entity)> = None;
    let mut consider = |t: f32, e: Entity| {
        if best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, e));
        }
    };
    for (e, gt, wall, state) in &targets.walls {
        let o = local_point(gt, origin);
        let d = local_dir(gt, dir);
        let half_t = state.grid.thickness * 0.5;
        let lo = Vec3::new(-wall.size[0] * 0.5, 0.0, -half_t);
        let hi = Vec3::new(wall.size[0] * 0.5, wall.size[1], half_t);
        if let Some(t) = ray_box(o, d, lo, hi, max) {
            consider(t, e);
        }
    }
    for (e, gt, _, state) in &targets.voxels {
        let o = local_point(gt, origin);
        let d = local_dir(gt, dir);
        if let Some((t, _, _)) = state.raycast(o, d, max) {
            consider(t, e);
        }
    }
    for (e, gt, piece, _) in &targets.pieces {
        let obb = Obb {
            center: gt.translation(),
            rotation: gt.rotation(),
            half: piece.half_extents,
        };
        if let Some(t) = obb.ray(origin, dir, max) {
            consider(t, e);
        }
    }
    for (e, gt, f) in &targets.fracturables {
        let obb = Obb {
            center: gt.translation(),
            rotation: gt.rotation(),
            half: f.shape.half_extents(),
        };
        if let Some(t) = obb.ray(origin, dir, max) {
            consider(t, e);
        }
    }
    best
}

fn ray_box(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3, max: f32) -> Option<f32> {
    let inv = d.recip();
    let t0 = (lo - o) * inv;
    let t1 = (hi - o) * inv;
    let tmin = t0.min(t1).max_element().max(0.0);
    let tmax = t0.max(t1).min_element().min(max);
    (tmin <= tmax && tmin.is_finite()).then_some(tmin)
}

#[allow(clippy::too_many_arguments)]
fn voxel_impact(
    ctx: &mut Ctx,
    gt: &GlobalTransform,
    volume: &VoxelVolume,
    state: &mut VoxelState,
    point: DVec3,
    dir: Vec3,
    energy: f32,
    min_radius: f32,
    melee: bool,
) {
    let props = ctx.table.get(volume.material);
    let lp = local_point(gt, point);
    let v = energy / props.toughness_j_m3;
    let r = (3.0 * v / (2.0 * std::f32::consts::PI))
        .cbrt()
        .max(if melee {
            min_radius.max(0.1)
        } else {
            min_radius.max(0.03)
        });
    let removed = state.crater(lp, r, ctx.rng.next_u64());
    if removed > 0.0 || r > 0.02 {
        let back = -dir;
        ctx.chips(
            point,
            back * 3.0,
            2.0,
            (r * 40.0) as usize + 1,
            volume.material,
            props.chip_size * 0.6,
        );
        ctx.dust(point, 0.3 + r, props.dust_darkness, 1, back);
        ctx.noise_events.push((point, 25.0));
    }
    ctx.stats.voxels_removed_m3 += removed.max(0.0);
}

/// An explosion: walls, voxels, pieces, props and loose debris within reach.
fn blast(
    ctx: &mut Ctx,
    commands: &mut Commands,
    targets: &mut Targets,
    center: DVec3,
    energy: f32,
) {
    let reach = (energy / (4.0 * std::f32::consts::PI * 2.0e4))
        .sqrt()
        .min(80.0) as f64;
    let rebar_cut = ctx.table.rebar_cut_flux;
    for (_, gt, wall, mut state) in &mut targets.walls {
        let local = local_point(gt, center);
        let w = wall.size[0] * 0.5;
        let near = Vec3::new(local.x.clamp(-w, w), local.y.clamp(0.0, wall.size[1]), 0.0);
        if (near - local).length() as f64 > reach {
            continue;
        }
        let props = ctx.table.get(wall.material);
        let gt = *gt;
        let mut hit = state.blast(&props, local, energy, rebar_cut);
        if let Some(brick) = wall.brick_size() {
            state.knock_out_bricks(brick, &mut hit);
        }
        if hit.volume > 0.0 {
            let side = if local.z >= 0.0 { 1.0 } else { -1.0 };
            let face = gt.transform_point(DVec3::new(
                local.x as f64,
                local.y as f64,
                (side * state.grid.thickness * 0.5) as f64,
            ));
            let size = (hit.volume.cbrt() * 2.0).clamp(0.4, 4.0);
            ctx.dust(
                face,
                size,
                props.dust_darkness,
                6,
                gt.rotation() * Vec3::Z * side,
            );
            let through = gt.transform_point(DVec3::new(
                local.x as f64,
                local.y as f64,
                (-side * state.grid.thickness) as f64,
            ));
            if !hit.lost.is_empty() {
                ctx.dust(
                    through,
                    size,
                    props.dust_darkness,
                    4,
                    gt.rotation() * Vec3::Z * -side * 2.0,
                );
            }
        }
        // Debris speed from the energy reaching the wall.
        let d = (near - local).length().max(0.2);
        let speed = ((energy / (4.0 * std::f32::consts::PI * d * d))
            / (props.density * state.grid.thickness))
            .sqrt()
            .clamp(2.0, BLAST_THROW_MAX);
        let wall = wall.clone();
        finish_wall_hit(ctx, &gt, &wall, &mut state, &hit, Some((local, speed)));
    }
    for (_, gt, volume, mut state) in &mut targets.voxels {
        let local = local_point(gt, center);
        let props = ctx.table.get(volume.material);
        let v = energy * 0.4 / props.toughness_j_m3;
        let r = (3.0 * v / (2.0 * std::f32::consts::PI)).cbrt();
        let ext = volume.extent();
        let lo = Vec3::new(-ext.x * 0.5, 0.0, -ext.z * 0.5);
        let hi = Vec3::new(ext.x * 0.5, ext.y, ext.z * 0.5);
        if (local.clamp(lo, hi) - local).length() > r {
            continue;
        }
        let removed = state.crater(local, r, ctx.rng.next_u64());
        if removed <= 0.0 {
            continue;
        }
        ctx.stats.voxels_removed_m3 += removed;
        let gt = *gt;
        // Rubble flying out of the crater.
        let pieces = ((removed / 0.03) as u32).clamp(2, ctx.settings.max_chunks_per_hit as u32);
        let half = Vec3::splat((removed / pieces as f32).cbrt() * 0.9).max(Vec3::splat(0.08));
        let (ext_c, int_c) = piece_colors(None, volume.material, ctx.table, ctx.textured);
        let shape = ConvexPoly::cuboid(half * 2.0);
        let parts = shatter(&shape, Vec3::ZERO, pieces, ctx.rng);
        let look = BreakLook::of(volume.material, ext_c, int_c);
        for (poly, c, vol) in parts {
            let dir = (c.normalize_or(Vec3::Y) + Vec3::Y * 0.6).normalize();
            let at = gt.transform_point((local + c * 0.5).as_dvec3());
            let out_dir = ((at - center).as_vec3()).normalize_or(dir);
            let speed = (energy / 1.0e5).sqrt().clamp(2.0, 18.0) * ctx.rng.range(0.5, 1.2);
            let (outside, inside) = poly.broken_meshes(c, &look, ctx.rng.next_u64());
            let pts: Vec<Vec3> = poly.vertices().iter().map(|p| *p - c).collect();
            ctx.pending.items.push(DebrisSpawn {
                position: at,
                rotation: Quat::IDENTITY,
                velocity: (out_dir + Vec3::Y * 0.5).normalize() * speed,
                angular: ctx.rng.unit_vector() * 8.0,
                look: DebrisLook::Broken {
                    outside: outside.map(MeshBuilder::build),
                    inside: inside.build(),
                },
                half_extents: half,
                hull: reduce_points(&pts, 16),
                mass_kg: vol * props.density,
                material: volume.material,
                shatter_speed: 0.0,
                lifetime: None,
                reuse: None,
                scale: None,
                resting: false,
                // Out of the crater: clear of the rock before it lands on it.
                from_rock: true,
            });
        }
        ctx.dust(
            center,
            (r * 2.0).clamp(0.5, 4.0),
            props.dust_darkness,
            6,
            Vec3::Y,
        );
    }
    // Pieces and props absorb energy over their exposed area; light ones break farther out.
    let reach = (energy / (4.0 * std::f32::consts::PI * 2.0e3))
        .sqrt()
        .min(120.0) as f64;
    let mut broken_pieces = Vec::new();
    for (e, gt, piece, mut state) in &mut targets.pieces {
        let obb = Obb {
            center: gt.translation(),
            rotation: gt.rotation(),
            half: piece.half_extents,
        };
        let d = obb.distance(center).max(0.15);
        if d as f64 > reach {
            continue;
        }
        let h = piece.half_extents;
        let area = 2.0 * (h.x * h.y + h.y * h.z + h.x * h.z);
        let flux = energy / (4.0 * std::f32::consts::PI * d * d);
        state.damage_j += flux * area;
        let props = ctx.table.get(piece.material);
        if state.damage_j >= piece.break_energy(&props) {
            let dir = (gt.translation() - center).as_vec3().normalize_or(Vec3::Y);
            let speed = (flux / (props.density * h.min_element() * 2.0))
                .sqrt()
                .clamp(1.0, 25.0);
            broken_pieces.push((e, *gt, piece.clone(), *state, dir, speed));
        }
    }
    for (e, gt, piece, state, dir, speed) in broken_pieces {
        break_piece(ctx, commands, e, &gt, &piece, &state, dir, speed);
    }
    let mut broken = Vec::new();
    for (e, gt, mut f) in &mut targets.fracturables {
        let obb = Obb {
            center: gt.translation(),
            rotation: gt.rotation(),
            half: f.shape.half_extents(),
        };
        let d = obb.distance(center).max(0.15);
        if d as f64 > reach {
            continue;
        }
        let h = f.shape.half_extents();
        let area = 2.0 * (h.x * h.y + h.y * h.z + h.x * h.z);
        let flux = energy / (4.0 * std::f32::consts::PI * d * d);
        f.damage_j += flux * area;
        let props = ctx.table.get(f.material);
        if f.damage_j >= f.break_energy(&props) {
            let impact = local_point(gt, center).clamp(-h, h);
            let dir = (gt.translation() - center).as_vec3().normalize_or(Vec3::Y);
            let speed = (flux / (props.density * h.min_element() * 2.0))
                .sqrt()
                .clamp(1.0, 25.0);
            broken.push((e, *gt, f.clone(), impact, dir, speed));
        }
    }
    for (e, gt, f, impact, dir, speed) in broken {
        break_fracturable(ctx, commands, e, &gt, &f, impact, dir, speed, Vec3::ZERO);
    }
    // Loose debris gets thrown.
    for (t, d, mut body) in &mut targets.debris {
        let off = (t.translation - center).as_vec3();
        let dist = off.length().max(0.3);
        if dist as f64 > reach {
            continue;
        }
        let flux = energy / (4.0 * std::f32::consts::PI * dist * dist);
        let area = (d.mass_kg / 2000.0).powf(2.0 / 3.0).max(1e-4);
        let dv = (2.0 * flux * area / d.mass_kg.max(0.01)).sqrt().min(30.0) * 0.5;
        let dir = (off / dist + Vec3::Y * 0.3).normalize();
        body.velocity += dir * dv;
        body.angular += Vec3::new(dir.z, 0.0, -dir.x) * dv;
        body.asleep = false;
        body.still = 0.0;
    }
}

#[allow(clippy::too_many_arguments)]
fn cut(
    ctx: &mut Ctx,
    targets: &mut Targets,
    wall: Entity,
    min: Vec2,
    max: Vec2,
    front: bool,
    melt: bool,
    tip: Vec3,
) {
    let Ok((_, gt, w, mut state)) = targets.walls.get_mut(wall) else {
        return;
    };
    let gt = *gt;
    let w = w.clone();
    let props = ctx.table.get(w.material);
    let hit = state.cut_rect(min, max);
    let t = state.grid.thickness;
    let mid = (min + max) * 0.5;
    let side = if front { 1.0 } else { -1.0 };
    let origin = Vec3::new(mid.x, mid.y, side * t * 2.0);
    if let Some(rect) = hit.rect {
        state.mark_dirty(rect);
    }
    ctx.stats.cells_destroyed += hit.lost.len() as u64;
    let first_piece = ctx.pending.items.len();
    if !melt {
        // The cut panel falls out as a few big slabs, pushed away from the charge.
        let per = (hit.lost.len() / 3).max(1);
        wall_chunks(
            ctx,
            &gt,
            &w,
            &state,
            &hit.lost,
            Some((origin, 4.0)),
            4,
            per,
            true,
        );
        let at = gt.transform_point(DVec3::new(mid.x as f64, mid.y as f64, (-side * t) as f64));
        ctx.dust(
            at,
            (max - min).max_element().clamp(0.5, 3.0),
            props.dust_darkness,
            6,
            gt.rotation() * Vec3::Z * -side,
        );
        ctx.noise_events.push((at, 60.0));
    } else if !hit.lost.is_empty() {
        // Burning metal: white smoke (for the smoke module), no dust.
        let at = gt.transform_point(DVec3::new(
            mid.x as f64,
            mid.y as f64,
            (side * t * 0.5) as f64,
        ));
        // Hot, so it rises off the cut in a plume instead of filling the bay.
        ctx.dust_events.push((at, 0.5, 0.05, 700.0));
    }
    let islands = state.detach_unsupported();
    for island in islands {
        // A freed panel tips out away from the side that was cut.
        let n = island.len() as f32;
        let c = island
            .iter()
            .map(|c| state.grid.center(c.idx))
            .sum::<Vec2>()
            / n;
        let push = Some((Vec3::new(c.x, c.y, side * t * 3.0), 2.0));
        // A cut steel plate comes out whole; other panels break in two.
        let per = if w.material == DestructibleMaterial::Metal {
            island.len()
        } else {
            (island.len() / 2).max(1)
        };
        wall_chunks(ctx, &gt, &w, &state, &island, push, 3, per, true);
        let at = gt.transform_point(DVec3::new(c.x as f64, c.y as f64, 0.0));
        ctx.dust(
            at,
            (n * state.grid.cell.x * state.grid.cell.y)
                .sqrt()
                .clamp(0.4, 3.0),
            props.dust_darkness,
            4,
            Vec3::ZERO,
        );
        ctx.noise_events.push((at, 50.0));
        ctx.stats.collapses += 1;
    }
    if tip != Vec3::ZERO {
        let throw = gt.rotation() * tip;
        for item in &mut ctx.pending.items[first_piece..] {
            item.velocity = throw;
        }
    }
}

fn dig(ctx: &mut Ctx, targets: &mut Targets, edit: &TerrainEdit, surfaces: &SurfaceMaterials) {
    for (_, gt, volume, mut state) in &mut targets.voxels {
        let c = local_point(gt, edit.center);
        let rot_inv = gt.rotation().inverse();
        let (lo, hi, sdf): (Vec3, Vec3, Box<dyn Fn(Vec3) -> f32>) = match edit.shape {
            EditShape::Sphere { radius } => (
                c - Vec3::splat(radius),
                c + Vec3::splat(radius),
                Box::new(move |p: Vec3| radius - p.distance(c)),
            ),
            EditShape::Box {
                half_extents,
                rotation,
            } => {
                let r = rot_inv * rotation;
                let ext = Vec3::splat(half_extents.length());
                (
                    c - ext,
                    c + ext,
                    Box::new(move |p: Vec3| box_sdf(p, c, half_extents, r)),
                )
            }
            EditShape::Capsule { to, radius } => {
                let b = c + rot_inv * to;
                (
                    c.min(b) - Vec3::splat(radius),
                    c.max(b) + Vec3::splat(radius),
                    Box::new(move |p: Vec3| capsule_sdf(p, c, b, radius)),
                )
            }
        };
        let ext = volume.extent();
        let vlo = Vec3::new(-ext.x * 0.5, 0.0, -ext.z * 0.5);
        let vhi = Vec3::new(ext.x * 0.5, ext.y, ext.z * 0.5);
        if hi.cmplt(vlo).any() || lo.cmpgt(vhi).any() {
            continue;
        }
        let material = edit
            .surface
            .and_then(|s| DestructibleMaterial::from_name(&surfaces.get(s).name))
            .unwrap_or(volume.material);
        let op = match edit.op {
            EditOp::Remove => VoxelOp::Remove,
            EditOp::Add => VoxelOp::Add(material),
            EditOp::Smooth { strength } => VoxelOp::Smooth(strength),
            EditOp::Paint => VoxelOp::Paint(material),
        };
        // Chips of what was dug out: soil under the grass, rock deeper in.
        let dug = match op {
            VoxelOp::Remove => state.material_in(lo, hi, &sdf),
            _ => None,
        }
        .unwrap_or(volume.material);
        let removed = state.edit(lo, hi, sdf, op);
        if removed > 0.0 {
            ctx.stats.voxels_removed_m3 += removed;
            let props: MaterialProps = ctx.table.get(dug);
            let at = edit.center;
            ctx.chips(
                at,
                Vec3::Y * 2.0,
                2.0,
                ((removed * 20.0) as usize).clamp(1, 8),
                dug,
                props.chip_size,
            );
            ctx.dust(
                at,
                (removed.cbrt() * 1.5).clamp(0.3, 2.5),
                props.dust_darkness,
                2,
                Vec3::Y * 0.5,
            );
        }
    }
}

/// Break a structural piece into fragments and remove it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn break_piece(
    ctx: &mut Ctx,
    commands: &mut Commands,
    e: Entity,
    gt: &GlobalTransform,
    piece: &StructuralPiece,
    state: &PieceState,
    dir: Vec3,
    speed: f32,
) {
    let f = Fracturable {
        shape: crate::structure::FractureShape::Box {
            half_extents: piece.half_extents,
        },
        material: piece.material,
        pieces: ((piece.half_extents.max_element() * 6.0) as u32).clamp(4, 12),
        color: piece.color,
        ..Fracturable::default()
    };
    let impact = state.last_hit.unwrap_or(Vec3::ZERO);
    break_fracturable(ctx, commands, e, gt, &f, impact, dir, speed, Vec3::ZERO);
    ctx.stats.pieces_broken += 1;
}

/// Wood this many times longer than it is thin breaks along its grain.
const GRAIN_ASPECT: f32 = 2.5;
/// How many times longer than wide its splinters run.
const SPLINTER_STRETCH: f32 = 4.0;

/// Shatter a fracturable into Voronoi pieces around `impact` (local) and remove it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn break_fracturable(
    ctx: &mut Ctx,
    commands: &mut Commands,
    e: Entity,
    gt: &GlobalTransform,
    f: &Fracturable,
    impact: Vec3,
    dir: Vec3,
    speed: f32,
    base_velocity: Vec3,
) {
    let props = ctx.table.get(f.material);
    let (ext, int) = piece_colors(f.color, f.material, ctx.table, ctx.textured);
    let half = f.shape.half_extents();
    // Long timber (posts, beams, planks) splits along its grain into splinters.
    let parts = if let crate::structure::FractureShape::Crate { half_extents, wall } = f.shape {
        crate_boards(half_extents, wall, ctx.rng)
    } else if f.material == DestructibleMaterial::Wood
        && half.max_element() >= GRAIN_ASPECT * half.min_element()
    {
        let grain = Vec3::AXES[half.max_position()];
        shatter_along_grain(
            &f.shape.poly(),
            impact,
            f.pieces,
            grain,
            SPLINTER_STRETCH,
            ctx.rng,
        )
    } else {
        shatter(&f.shape.poly(), impact, f.pieces, ctx.rng)
    };
    let rot = gt.rotation();
    let size = half.max_element() * 2.0;
    let look = BreakLook::of(f.material, ext, int);
    for (poly, c, vol) in parts {
        let (outside, inside) = poly.broken_meshes(c, &look, ctx.rng.next_u64());
        let pts: Vec<Vec3> = poly.vertices().iter().map(|p| *p - c).collect();
        let (lo, hi) = poly.bounds();
        let dist = c.distance(impact);
        let falloff = 1.0 / (1.0 + 2.0 * dist / size.max(0.1));
        let outward = rot * (c - impact).normalize_or(Vec3::Y);
        let velocity =
            base_velocity + (dir * 0.6 + outward * 0.4) * speed * falloff * ctx.rng.range(0.6, 1.3);
        ctx.pending.items.push(DebrisSpawn {
            position: gt.transform_point(c.as_dvec3()),
            rotation: rot,
            velocity,
            angular: ctx.rng.unit_vector() * (speed * falloff * 2.0).min(12.0),
            look: DebrisLook::Broken {
                outside: outside.map(MeshBuilder::build),
                inside: inside.build(),
            },
            half_extents: (hi - lo) * 0.5,
            hull: reduce_points(&pts, 16),
            mass_kg: vol * props.density,
            material: f.material,
            shatter_speed: 0.0,
            lifetime: None,
            reuse: None,
            scale: None,
            resting: false,
            from_rock: false,
        });
    }
    let at = gt.translation();
    ctx.dust(
        at,
        (size * 0.8).clamp(0.3, 3.0),
        props.dust_darkness,
        3,
        Vec3::ZERO,
    );
    ctx.noise_events.push((at, 35.0));
    commands.entity(e).despawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_box_enters_front_face() {
        let t = ray_box(
            Vec3::new(0.0, 1.0, 5.0),
            Vec3::NEG_Z,
            Vec3::new(-1.0, 0.0, -0.1),
            Vec3::new(1.0, 2.0, 0.1),
            10.0,
        )
        .unwrap();
        assert!((t - 4.9).abs() < 1e-5);
        assert!(
            ray_box(
                Vec3::new(5.0, 1.0, 5.0),
                Vec3::NEG_Z,
                Vec3::splat(-1.0),
                Vec3::ONE,
                10.0
            )
            .is_none()
        );
    }
}
