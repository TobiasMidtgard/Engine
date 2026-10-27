//! Things giving way: building pieces that lose support fall, voxel overhangs break off, and
//! heavy chunks shatter when they land.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy_tasks::{Task, block_on, poll_once};

use engine_core::math::{DVec3, Rng, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{ContactEvent, ContactPhase};
use engine_world::{Combustible, Env, WorldImpulse};

use crate::DestructionSettings;
use crate::assets::bake_pool;
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

/// What [`structure_system`] keeps between steps: the ground height under each piece's center
/// (by its exact position), and the pieces counting down to fall.
#[derive(Default)]
pub struct SupportCache {
    ground: std::collections::HashMap<[u64; 3], f64>,
    counting: Vec<Entity>,
}

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
        Entity,
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
    rock_changed: Query<(&GlobalTransform, &VoxelState), Changed<VoxelState>>,
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
    mut cache: Local<SupportCache>,
) {
    let rng = rng.get_or_insert_with(|| Rng::new(0xC011_A95E));
    // Where something changed: the old and new boxes of moved, added, removed and weakened
    // pieces, and the bounds of rock that was dug or filled.
    let mut regions: Vec<(DVec3, DVec3)> = Vec::new();
    let mut seeds: HashSet<Entity> = HashSet::new();
    let old_box = |graph: &StructureGraph, e: Entity| {
        graph
            .index
            .get(&e)
            .map(|i| graph.boxes[*i].aabb(REGION_MARGIN))
    };
    for e in changed.iter().chain(std::mem::take(&mut graph.touched)) {
        regions.extend(old_box(&graph, e));
        seeds.insert(e);
    }
    for e in removed.read() {
        regions.extend(old_box(&graph, e));
    }
    for (gt, state) in &rock_changed {
        let (lo, hi) = rock_bounds(gt, state);
        // Rock dug out from under a piece (or added under it) changes what stands there.
        cache.ground.retain(|k, _| {
            let p = DVec3::new(
                f64::from_bits(k[0]),
                f64::from_bits(k[1]),
                f64::from_bits(k[2]),
            );
            // Anything over the rock's footprint stands on it (or on what is under it).
            !(p.x >= lo.x && p.x <= hi.x && p.z >= lo.z && p.z <= hi.z && p.y >= lo.y)
        });
        regions.push((lo, hi));
    }
    let dt = time.delta_secs();
    if graph.dirty || !regions.is_empty() || !seeds.is_empty() {
        let _span = tracing::info_span!("support graph rebuild").entered();
        let full = std::mem::take(&mut graph.dirty);
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
        for (e, obb, _) in &items {
            if seeds.contains(e) || !graph.index.contains_key(e) {
                regions.push(obb.aabb(REGION_MARGIN));
            }
        }
        let hit = |(lo, hi): (DVec3, DVec3)| {
            regions
                .iter()
                .any(|(a, b)| lo.cmple(*b).all() && hi.cmpge(*a).all())
        };
        // The islands a change touches are re-solved whole; the rest keep their support.
        let affected: HashSet<u32> = if full {
            HashSet::new()
        } else {
            graph
                .boxes
                .iter()
                .zip(&graph.island)
                .filter(|(b, _)| hit(b.aabb(REGION_MARGIN)))
                .map(|(_, i)| *i)
                .collect()
        };
        let redo = |e: &Entity, obb: &Obb| {
            full || match graph.index.get(e) {
                Some(&i) => affected.contains(&graph.island[i]) || seeds.contains(e),
                None => true,
            } || hit(obb.aabb(REGION_MARGIN))
        };
        let (sub, kept): (Vec<_>, Vec<_>) = items.into_iter().partition(|(e, obb, _)| redo(e, obb));
        let kept: HashSet<Entity> = kept.into_iter().map(|(e, ..)| e).collect();

        // Pieces stand on the terrain or on voxel rock.
        let rock: Vec<(&GlobalTransform, &VoxelState)> = rock.iter().collect();
        // The ground under a piece that hasn't moved is what it was at the last rebuild
        // (sampling the terrain under thousands of pieces was most of a rebuild).
        if cache.ground.len() > 4 * (sub.len() + kept.len()) + 256 {
            cache.ground.clear();
        }
        let heights = std::cell::RefCell::new(std::mem::take(&mut cache.ground));
        let ground = |p: DVec3| {
            let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
            if let Some(h) = heights.borrow().get(&key) {
                return *h;
            }
            let terrain = env.ground(p).map(|g| g.height).unwrap_or(f64::NEG_INFINITY);
            let h = rock_below(&rock, p).map_or(terrain, |r| r.max(terrain));
            heights.borrow_mut().insert(key, h);
            h
        };
        let g = build_graph(&sub, ground);
        cache.ground = heights.into_inner();
        let mut props: Vec<MaterialProps> = Vec::with_capacity(sub.len());
        for (e, ..) in &sub {
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
        let redone: HashSet<Entity> = sub.iter().map(|(e, ..)| *e).collect();
        cache.counting.retain(|e| kept.contains(e));
        for (i, (e, ..)) in sub.iter().enumerate() {
            if let Ok((.., mut state, _, _)) = pieces.get_mut(*e) {
                state.support = support[i];
                state.max_support = props[i].max_support;
                let failing = support[i] < props[i].min_support || props[i].max_support <= 0.0;
                if failing && state.collapse_in.is_none() {
                    state.collapse_in = Some(settings.collapse_delay_secs * rng.range(0.4, 1.6));
                } else if !failing {
                    state.collapse_in = None;
                }
                if state.collapse_in.is_some() {
                    cache.counting.push(*e);
                }
            }
        }
        *graph = merge_graphs(std::mem::take(&mut *graph), &kept, g);
        debug_assert!(
            graph
                .pieces
                .iter()
                .all(|e| kept.contains(e) || redone.contains(e))
        );
    }

    // Count down and drop: only the pieces the last rebuild found failing, not every piece.
    let mut fallen: Vec<(Entity, DVec3, f32)> = Vec::new();
    let mut dust_darkness = 0.0;
    let mut counting = std::mem::take(&mut cache.counting);
    counting.retain(|e| {
        pieces
            .get(*e)
            .is_ok_and(|(.., state, _, _)| state.collapse_in.is_some())
    });
    for &e in &counting {
        let Ok((e, t, gt, parent, piece, mut state, fire, charring)) = pieces.get_mut(e) else {
            continue;
        };
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
            from_rock: false,
        });
        fallen.push((e, translation, mass));
        dust_darkness = props.dust_darkness;
    }
    cache.counting = counting;
    if !fallen.is_empty() {
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

/// How far around a change pieces count as touched by it, m (more than the graph's contact
/// margin, so a piece that now touches a moved one is re-solved with it).
const REGION_MARGIN: f32 = 0.1;

/// World bounds of a voxel volume's grid.
fn rock_bounds(gt: &GlobalTransform, state: &VoxelState) -> (DVec3, DVec3) {
    let ext = Vec3::new(
        state.dims[0] as f32,
        state.dims[1] as f32,
        state.dims[2] as f32,
    ) * state.size;
    let affine = gt.affine();
    let mut lo = DVec3::splat(f64::INFINITY);
    let mut hi = DVec3::splat(f64::NEG_INFINITY);
    for c in 0..8 {
        let corner = state.origin
            + Vec3::new(
                if c & 1 == 0 { 0.0 } else { ext.x },
                if c & 2 == 0 { 0.0 } else { ext.y },
                if c & 4 == 0 { 0.0 } else { ext.z },
            );
        let p = affine.transform_point3(corner.as_dvec3());
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let m = DVec3::splat(REGION_MARGIN as f64);
    (lo - m, hi + m)
}

/// `old` without the pieces outside `kept`, with `fresh` (the re-solved islands) added. Kept
/// pieces keep their edges: an island is kept or re-solved whole, so no edge joins the two.
fn merge_graphs(
    old: StructureGraph,
    kept: &HashSet<Entity>,
    fresh: StructureGraph,
) -> StructureGraph {
    let mut map = vec![usize::MAX; old.pieces.len()];
    let mut out = StructureGraph::default();
    for (i, e) in old.pieces.iter().enumerate() {
        if kept.contains(e) {
            map[i] = out.pieces.len();
            out.pieces.push(*e);
        }
    }
    let mut next_island = 0;
    for i in 0..old.pieces.len() {
        if map[i] == usize::MAX {
            continue;
        }
        out.edges.push(
            old.edges[i]
                .iter()
                .filter(|(j, ..)| map[*j] != usize::MAX)
                .map(|&(j, v, h)| (map[j], v, h))
                .collect(),
        );
        out.grounded.push(old.grounded[i]);
        out.boxes.push(old.boxes[i]);
        out.island.push(old.island[i]);
        next_island = next_island.max(old.island[i] + 1);
    }
    let base = out.pieces.len();
    out.pieces.extend(&fresh.pieces);
    out.edges.extend(
        fresh
            .edges
            .into_iter()
            .map(|es| es.into_iter().map(|(j, v, h)| (j + base, v, h)).collect()),
    );
    out.grounded.extend(fresh.grounded);
    out.boxes.extend(fresh.boxes);
    out.island
        .extend(fresh.island.into_iter().map(|i| i + next_island));
    out.index = out
        .pieces
        .iter()
        .enumerate()
        .map(|(i, e)| (*e, i))
        .collect();
    out.touched = old.touched;
    out
}

/// Support solves of dug voxel volumes running on the bake threads, by volume: a solve walks
/// the whole grid (a few milliseconds for the fortress cliff, every dig), so it runs beside the
/// game, the way Teardown and Red Faction resolve structural checks off the frame. Digs made
/// meanwhile wait for the next solve; one that added rock throws the result away.
#[derive(Resource, Default)]
pub struct VoxelSupportJobs {
    pending: HashMap<Entity, (u32, Task<Vec<Vec<(usize, u8, u8)>>>)>,
}

impl VoxelSupportJobs {
    /// Forget `volume`'s solve (its state was replaced).
    pub fn cancel(&mut self, volume: Entity) {
        self.pending.remove(&volume);
    }
}

/// Cut out voxels that lost support (Valheim-style) and drop them as falling rock. The solve
/// runs on the bake threads ([`VoxelSupportJobs`]) unless [`crate::LoadAtOnce`] is present.
#[allow(clippy::too_many_arguments)]
pub fn voxel_support_system(
    table: Res<DestructionMaterials>,
    settings: Res<DestructionSettings>,
    at_once: Option<Res<crate::LoadAtOnce>>,
    mut jobs: ResMut<VoxelSupportJobs>,
    mut volumes: Query<(
        Entity,
        &GlobalTransform,
        &Transform,
        Option<&ChildOf>,
        &VoxelVolume,
        &mut VoxelState,
    )>,
    mut out: Outputs,
    mut rng: Local<Option<Rng>>,
    mut shared: Local<Option<Arc<DestructionMaterials>>>,
) {
    let rng = rng.get_or_insert_with(|| Rng::new(0x0FA1_1E57));
    jobs.pending.retain(|e, _| volumes.contains(*e));
    for (e, gt, t, parent, volume, mut state) in &mut volumes {
        let groups = if at_once.is_some() {
            if !state.support_dirty {
                continue;
            }
            state.detach_unsupported(&table)
        } else if let Some((gains, task)) = jobs.pending.get_mut(&e) {
            let Some(groups) = block_on(poll_once(task)) else {
                continue;
            };
            let stale = *gains != state.gains;
            jobs.pending.remove(&e);
            if stale {
                state.support_dirty = true;
                continue;
            }
            state.remove_groups(groups)
        } else {
            if !state.support_dirty {
                continue;
            }
            state.support_dirty = false;
            let snapshot = state.support_snapshot();
            let table = shared
                .get_or_insert_with(|| Arc::new(table.clone()))
                .clone();
            jobs.pending.insert(
                e,
                (
                    snapshot.gains,
                    bake_pool().spawn(async move { snapshot.unsupported_groups(&table) }),
                ),
            );
            continue;
        };
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
                let volume_m3 = part.len() as f32 * voxel_m3;
                if part.len() < 2 {
                    let at = affine.transform_point(com.as_dvec3());
                    let mut ctx = ctx(&settings, &table, rng, &mut out);
                    ctx.chips(at, Vec3::ZERO, 1.0, 2, volume.material, state.size);
                    continue;
                }
                let (velocity, angular) = (rng.unit_vector() * 0.2, rng.unit_vector() * 0.25);
                let (look, size, material) = (look.clone(), state.size, volume.material);
                let mass_kg = volume_m3 * props.density;
                // Meshing the boulder is the slow part: on the bake threads.
                ctx(&settings, &table, rng, &mut out).later(move || {
                    let mb = mesh_all(&piece, &look, com);
                    if mb.is_empty() {
                        return Vec::new();
                    }
                    let pts: Vec<Vec3> =
                        mb.vertices.iter().map(|v| Vec3::from(v.position)).collect();
                    let (lo, hi) = pts.iter().fold(
                        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                        |(lo, hi), p| (lo.min(*p), hi.max(*p)),
                    );
                    vec![DebrisSpawn {
                        position: affine.transform_point(com.as_dvec3()),
                        rotation: affine.rotation(),
                        velocity,
                        angular,
                        look: DebrisLook::Mesh(mb.build()),
                        half_extents: ((hi - lo) * 0.5).max(Vec3::splat(size * 0.5)),
                        hull: reduce_points(&pts, 24),
                        mass_kg,
                        material,
                        // Boulders keep their rock shape: they land and roll, they don't
                        // burst into boxes.
                        shatter_speed: 0.0,
                        lifetime: None,
                        reuse: None,
                        scale: None,
                        resting: false,
                        from_rock: true,
                    }]
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
        defer: out.defer(),
        rng,
        pending: &mut out.pending,
        stats: &mut out.stats,
        dust_events: Vec::new(),
        noise_events: Vec::new(),
    }
}

/// Thinnest half extent (m) of a landing piece that still bursts into pieces; thinner ones
/// crumble into chips and dust.
const CRUMBLE_HALF_M: f32 = 0.08;

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
        let carry = body.map(|b| b.velocity * 0.4).unwrap_or(Vec3::ZERO);
        let mut ctx = ctx(&settings, &table, rng, &mut out);
        // Both ways below despawn the piece; free its mesh first.
        if let (Some(m), Some(meshes)) = (mesh, meshes.as_deref_mut()) {
            m.free(meshes);
        }
        // A thin slab (a plaster sheet) breaks up into grit and a puff, not into smaller slabs.
        if s.half_extents.min_element() < CRUMBLE_HALF_M {
            let props = table.get(d.material);
            commands.entity(e).despawn();
            ctx.chips(
                t.translation,
                carry + Vec3::Y * 1.5,
                2.0,
                8,
                d.material,
                props.chip_size,
            );
            ctx.dust(
                t.translation,
                s.half_extents.max_element() * 2.0,
                props.dust_darkness,
                2,
                carry * 0.5,
            );
            ctx.stats.pieces_broken += 1;
            continue;
        }
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
