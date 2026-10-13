//! Things giving way: building pieces that lose support fall, voxel overhangs break off, and
//! heavy chunks shatter when they land.

use engine_core::math::{DVec3, Rng, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{ContactEvent, ContactPhase};
use engine_world::{Combustible, Env, WorldImpulse};

use crate::DestructionSettings;
use crate::burn::{Charring, burn_strength, fuel_left};
use crate::debris::{
    Debris, DebrisBody, DebrisLook, DebrisMesh, DebrisSpawn, ShatterOnImpact, reduce_points,
};
use crate::hits::{Ctx, Outputs, break_fracturable};
use crate::material::{DestructibleMaterial, DestructionMaterials, MaterialProps};
use crate::structure::{
    Fracturable, FractureShape, Obb, PieceState, StructuralPiece, StructureGraph, build_graph,
    solve_support,
};
use crate::voxel::{VoxelLook, VoxelState, VoxelVolume, mesh_all, rock_below};

/// Keep the support graph of [`StructuralPiece`]s current, update every [`PieceState`], and
/// drop pieces that lost support (after a short, slightly random delay so collapses cascade).
#[allow(clippy::too_many_arguments)]
pub fn structure_system(
    mut commands: Commands,
    time: Res<Time>,
    env: Env,
    table: Res<DestructionMaterials>,
    settings: Res<DestructionSettings>,
    mut graph: ResMut<StructureGraph>,
    mut out: Outputs,
    mut impulses: MessageWriter<WorldImpulse>,
    changed: Query<
        (),
        (
            With<StructuralPiece>,
            Or<(
                Added<StructuralPiece>,
                Changed<GlobalTransform>,
                Changed<Transform>,
            )>,
        ),
    >,
    mut removed: RemovedComponents<StructuralPiece>,
    rock: Query<(&GlobalTransform, &VoxelState)>,
    rock_changed: Query<(), Changed<VoxelState>>,
    mut pieces: Query<(
        Entity,
        &Transform,
        &GlobalTransform,
        Option<&ChildOf>,
        &StructuralPiece,
        &mut PieceState,
        Option<&Combustible>,
        Option<&Charring>,
    )>,
    mut rng: Local<Option<Rng>>,
) {
    let rng = rng.get_or_insert_with(|| Rng::new(0xC011_A95E));
    // Rock dug out from under a piece (or added under it) changes what stands on the ground.
    if !changed.is_empty() || removed.read().count() > 0 || !rock_changed.is_empty() {
        graph.dirty = true;
    }
    let dt = time.delta_secs();
    if graph.dirty {
        graph.dirty = false;
        let items: Vec<(Entity, Obb, bool)> = pieces
            .iter()
            .map(|(e, t, gt, parent, piece, ..)| {
                // Root pieces read Transform so a structure is right on its first step, before
                // transforms are propagated.
                let (center, rotation) = if parent.is_none() {
                    (t.translation, t.rotation)
                } else {
                    (gt.translation(), gt.rotation())
                };
                (
                    e,
                    Obb {
                        center,
                        rotation,
                        half: piece.half_extents,
                    },
                    piece.foundation,
                )
            })
            .collect();
        // Pieces stand on the terrain or on voxel rock.
        let rock: Vec<(&GlobalTransform, &VoxelState)> = rock.iter().collect();
        let ground = |p: DVec3| {
            let terrain = env.ground(p).map(|g| g.height).unwrap_or(f64::NEG_INFINITY);
            rock_below(&rock, p).map_or(terrain, |r| r.max(terrain))
        };
        let mut g = build_graph(&items, ground);
        let mut props: Vec<MaterialProps> = Vec::with_capacity(items.len());
        for (e, ..) in &items {
            let Ok((_, _, _, _, piece, _, fire, charring)) = pieces.get(*e) else {
                props.push(table.get(DestructibleMaterial::Wood));
                continue;
            };
            let mut p = table.get(piece.material);
            let k = burn_strength(piece, &p, fire, charring);
            p.max_support *= k;
            props.push(p);
        }
        let support = solve_support(&g, &props);
        for (i, (e, ..)) in items.iter().enumerate() {
            if let Ok((.., mut state, _, _)) = pieces.get_mut(*e) {
                state.support = support[i];
                state.max_support = props[i].max_support;
                let failing = support[i] < props[i].min_support || props[i].max_support <= 0.0;
                if failing && state.collapse_in.is_none() {
                    state.collapse_in = Some(settings.collapse_delay_secs * rng.range(0.4, 1.6));
                } else if !failing {
                    state.collapse_in = None;
                }
            }
        }
        g.dirty = false;
        *graph = g;
    }

    // Count down and drop.
    let mut fallen: Vec<(Entity, DVec3, f32)> = Vec::new();
    let mut dust_darkness = 0.0;
    for (e, t, gt, parent, piece, mut state, fire, charring) in &mut pieces {
        let Some(left) = state.collapse_in.as_mut() else {
            continue;
        };
        *left -= dt;
        if *left > 0.0 {
            continue;
        }
        let props = table.get(piece.material);
        let (translation, rotation) = if parent.is_none() {
            (t.translation, t.rotation)
        } else {
            (gt.translation(), gt.rotation())
        };
        let scale = if parent.is_none() {
            t.scale
        } else {
            gt.scale()
        };
        let h = piece.half_extents;
        // Burnt wood has lost most of its mass to the fire.
        let mass = 8.0
            * h.x
            * h.y
            * h.z
            * props.density
            * (0.2 + 0.8 * fuel_left(piece, &props, fire, charring));
        let mut ec = commands.entity(e);
        ec.remove::<(StructuralPiece, PieceState)>();
        if parent.is_some() {
            ec.remove::<ChildOf>();
        }
        let corners: Vec<Vec3> = [-1.0f32, 1.0]
            .iter()
            .flat_map(|x| {
                [-1.0f32, 1.0]
                    .iter()
                    .flat_map(move |y| [-1.0f32, 1.0].map(move |z| Vec3::new(*x, *y, z)))
            })
            .map(|c| c * h / scale.max(Vec3::splat(1e-3)))
            .collect();
        out.pending.items.push(DebrisSpawn {
            position: translation,
            rotation,
            velocity: rng.unit_vector() * 0.3,
            angular: rng.unit_vector() * 0.4,
            look: DebrisLook::Keep,
            half_extents: h,
            hull: corners,
            mass_kg: mass,
            material: piece.material,
            shatter_speed: props.shatter_speed,
            lifetime: None,
            reuse: Some(e),
            scale: Some(scale),
            resting: false,
        });
        fallen.push((e, translation, mass));
        dust_darkness = props.dust_darkness;
    }
    if !fallen.is_empty() {
        graph.dirty = true;
        out.stats.collapses += 1;
        let total: f32 = fallen.iter().map(|f| f.2).sum();
        let center = fallen.iter().map(|f| f.1).sum::<DVec3>() / fallen.len() as f64;
        let size = (total / 500.0).cbrt().clamp(0.4, 4.0);
        out.pending.dust.push(crate::debris::DustRequest {
            position: center,
            velocity: Vec3::ZERO,
            size,
            darkness: dust_darkness,
            count: (fallen.len() as u32 * 2).min(10),
        });
        out.noise.write(engine_world::NoiseEvent {
            position: center,
            radius: (20.0 * size).clamp(10.0, 120.0),
            source: None,
        });
        if total > 300.0 {
            // The air a big collapse pushes out (dust, foliage, smoke react).
            impulses.write(WorldImpulse {
                center,
                radius: size * 3.0,
                strength: (total.sqrt() * 0.15).clamp(2.0, 25.0),
                energy_j: 0.0,
                heat_c: 0.0,
                source: None,
            });
        }
    }
}

/// Cut out voxels that lost support (Valheim-style) and drop them as falling rock.
pub fn voxel_support_system(
    table: Res<DestructionMaterials>,
    settings: Res<DestructionSettings>,
    mut volumes: Query<(
        &GlobalTransform,
        &Transform,
        Option<&ChildOf>,
        &VoxelVolume,
        &mut VoxelState,
    )>,
    mut out: Outputs,
    mut rng: Local<Option<Rng>>,
) {
    let rng = rng.get_or_insert_with(|| Rng::new(0x0FA1_1E57));
    for (gt, t, parent, volume, mut state) in &mut volumes {
        if !state.support_dirty {
            continue;
        }
        let groups = state.detach_unsupported(&table);
        if groups.is_empty() {
            continue;
        }
        let affine = if parent.is_none() {
            GlobalTransform::from(*t)
        } else {
            *gt
        };
        let look = VoxelLook::new(volume, &table).with_layers(out.textured());
        let props = table.get(volume.material);
        let voxel_m3 = state.size.powi(3);
        let mut lost = 0.0;
        for group in groups {
            lost += group.len() as f32 * voxel_m3;
            // Big overhangs break into boulders of a few cubic meters that fall together.
            let group_m3 = group.len() as f32 * voxel_m3;
            let k = ((group_m3 / 2.5).ceil() as usize).clamp(1, settings.max_chunks_per_hit.max(1));
            let centers: Vec<Vec3> = group
                .iter()
                .map(|(idx, ..)| {
                    let (i, j, k) = state.coords(*idx);
                    state.center(i, j, k)
                })
                .collect();
            let seeds = spread_seeds(&centers, k, rng);
            let mut parts: Vec<Vec<(usize, u8, u8)>> = vec![Vec::new(); seeds.len()];
            // Bend the boundaries between boulders with smooth noise rather than per-voxel
            // jitter: per-voxel jitter interleaves stray voxels along every boundary, and the
            // boulders come out with stringy, melted-looking spurs.
            let noise_seed = rng.next_u64();
            let wobble = |c: Vec3, k: u64| {
                let q = c.as_dvec3() * 0.35;
                engine_core::math::noise::perlin3(q.x, q.y, q.z, noise_seed.wrapping_add(k)) as f32
            };
            for (v, c) in group.iter().zip(&centers) {
                let jitter = Vec3::new(wobble(*c, 1), wobble(*c, 2), wobble(*c, 3)) * 1.6;
                let best = seeds
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        a.1.distance_squared(*c + jitter)
                            .total_cmp(&b.1.distance_squared(*c + jitter))
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                parts[best].push(*v);
            }
            for part in parts {
                if part.is_empty() {
                    continue;
                }
                let piece = state.piece(&part);
                let com = part
                    .iter()
                    .map(|(idx, ..)| {
                        let (i, j, k) = state.coords(*idx);
                        state.center(i, j, k)
                    })
                    .sum::<Vec3>()
                    / part.len() as f32;
                let mb = mesh_all(&piece, &look, com);
                let volume_m3 = part.len() as f32 * voxel_m3;
                if mb.is_empty() || part.len() < 2 {
                    let at = affine.transform_point(com.as_dvec3());
                    let mut ctx = ctx(&settings, &table, rng, &mut out);
                    ctx.chips(at, Vec3::ZERO, 1.0, 2, volume.material, state.size);
                    continue;
                }
                let pts: Vec<Vec3> = mb.vertices.iter().map(|v| Vec3::from(v.position)).collect();
                let (lo, hi) = pts.iter().fold(
                    (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                    |(lo, hi), p| (lo.min(*p), hi.max(*p)),
                );
                out.pending.items.push(DebrisSpawn {
                    position: affine.transform_point(com.as_dvec3()),
                    rotation: affine.rotation(),
                    velocity: rng.unit_vector() * 0.2,
                    angular: rng.unit_vector() * 0.25,
                    look: DebrisLook::Mesh(mb.build()),
                    half_extents: ((hi - lo) * 0.5).max(Vec3::splat(state.size * 0.5)),
                    hull: reduce_points(&pts, 24),
                    mass_kg: volume_m3 * props.density,
                    material: volume.material,
                    // Boulders keep their rock shape: they land and roll, they don't burst into boxes.
                    shatter_speed: 0.0,
                    lifetime: None,
                    reuse: None,
                    scale: None,
                    resting: false,
                });
            }
            let at = affine
                .transform_point(centers.iter().sum::<Vec3>().as_dvec3() / centers.len() as f64);
            let size = (group.len() as f32 * voxel_m3).cbrt().clamp(0.3, 4.0);
            let mut ctx = ctx(&settings, &table, rng, &mut out);
            ctx.dust(at, size, props.dust_darkness, 4, Vec3::ZERO);
        }
        out.stats.collapses += 1;
        out.stats.voxels_removed_m3 += lost;
    }
}

/// `k` seed points spread through `points` (farthest-point sampling), so Voronoi pieces come
/// out about the same size instead of a few slivers and one giant.
fn spread_seeds(points: &[Vec3], k: usize, rng: &mut Rng) -> Vec<Vec3> {
    let mut seeds = vec![points[rng.range_i32(0, points.len() as i32) as usize]];
    let mut dist: Vec<f32> = points
        .iter()
        .map(|p| p.distance_squared(seeds[0]))
        .collect();
    while seeds.len() < k {
        let Some((far, d)) = dist.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
            break;
        };
        if *d <= 0.0 {
            break;
        }
        let seed = points[far];
        seeds.push(seed);
        for (d, p) in dist.iter_mut().zip(points) {
            *d = d.min(p.distance_squared(seed));
        }
    }
    seeds
}

fn ctx<'a>(
    settings: &'a DestructionSettings,
    table: &'a DestructionMaterials,
    rng: &'a mut Rng,
    out: &'a mut Outputs,
) -> Ctx<'a> {
    Ctx {
        settings,
        table,
        textured: out.textured(),
        rng,
        pending: &mut out.pending,
        stats: &mut out.stats,
        dust_events: Vec::new(),
        noise_events: Vec::new(),
    }
}

/// Debris marked [`ShatterOnImpact`] breaks into Voronoi pieces when it lands hard (stone and
/// concrete slabs burst on the ground; wooden beams don't).
#[allow(clippy::too_many_arguments)]
pub fn shatter_system(
    mut commands: Commands,
    table: Res<DestructionMaterials>,
    settings: Res<DestructionSettings>,
    mut out: Outputs,
    mut contacts: MessageReader<ContactEvent>,
    q: Query<(
        Entity,
        &Transform,
        &Debris,
        Option<&DebrisBody>,
        &ShatterOnImpact,
        Option<&DebrisMesh>,
    )>,
    mut meshes: Option<ResMut<engine_assets::Assets<engine_render::Mesh>>>,
    mut rng: Local<Option<Rng>>,
) {
    let rng = rng.get_or_insert_with(|| Rng::new(0x5A77_E125));
    let mut hard: Vec<(Entity, f32)> = Vec::new();
    for c in contacts.read() {
        if c.phase != ContactPhase::Started {
            continue;
        }
        for e in [c.a, c.b] {
            if let Ok((_, _, d, ..)) = q.get(e) {
                hard.push((e, c.impulse / d.mass_kg.max(0.01)));
            }
        }
    }
    for (e, _, _, body, s, _) in &q {
        if let Some(b) = body
            && !b.sinking
            && b.last_impact > s.speed
        {
            hard.push((e, b.last_impact));
        }
    }
    if hard.is_empty() {
        return;
    }
    let mut done: Vec<Entity> = Vec::new();
    for (e, speed) in hard {
        if done.contains(&e) {
            continue;
        }
        let Ok((_, t, d, body, s, mesh)) = q.get(e) else {
            continue;
        };
        if speed < s.speed || d.age > d.lifetime {
            continue;
        }
        done.push(e);
        let half = s.half_extents * 0.8;
        let pieces = ((8.0 * half.x * half.y * half.z).cbrt() * 10.0) as u32;
        let f = Fracturable {
            shape: FractureShape::Box { half_extents: half },
            material: d.material,
            pieces: pieces.clamp(3, 9),
            ..Fracturable::default()
        };
        let gt = GlobalTransform::from(Transform {
            translation: t.translation,
            rotation: t.rotation,
            scale: Vec3::ONE,
        });
        let carry = body.map(|b| b.velocity * 0.4).unwrap_or(Vec3::ZERO);
        let mut ctx = ctx(&settings, &table, rng, &mut out);
        // `break_fracturable` despawns the entity; free its mesh first.
        if let (Some(m), Some(meshes)) = (mesh, meshes.as_deref_mut()) {
            m.free(meshes);
        }
        let impact = Vec3::new(0.0, -half.y, 0.0);
        break_fracturable(
            &mut ctx,
            &mut commands,
            e,
            &gt,
            &f,
            impact,
            Vec3::Y,
            (speed * 0.35).min(5.0),
            carry,
        );
        ctx.chips(
            t.translation,
            Vec3::Y * 2.0,
            2.5,
            6,
            d.material,
            table.get(d.material).chip_size,
        );
        ctx.stats.pieces_broken += 1;
    }
}
