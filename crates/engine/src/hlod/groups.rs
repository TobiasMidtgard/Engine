//! The second proxy level (Unreal's HLOD1): far away, the proxies of every out cell within one
//! [`HlodSettings::group_m`] square merge into a single coarser mesh.
//!
//! A valley of 128 m cells seen from a mountain is a hundred proxies, a hundred draws of a few
//! thousand triangles each, most of them too far away for their detail to show. Unreal's World
//! Partition answers with HLOD layers in a hierarchy, each with its own cell size and loading
//! range: past the first layer's range the parent layer's merged actor stands in for its
//! children. Here the members' proxy meshes are re-clustered on a worker into one mesh of at
//! most [`HlodSettings::group_triangles`] (the same vertex clustering, on a coarser grid), and
//! the group shows once every source is [`HlodSettings::group_from_m`] from it and every member
//! is out, cross-fading with its members' proxies over [`HlodSettings::group_fade_s`]. It
//! draws farther than a cell's proxy ([`HlodSettings::group_draw_m`]). Groups of cells whose
//! proxies are kept on disk are kept too, under a hash of their members' versions.
//!
//! Cells riding a [`StreamFrame`](engine_world::StreamFrame) stay out of groups (their frames
//! move apart). Groups are made from where the cells are when the set of cells changes.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use super::*;
use engine_world::StreamedCells;

/// A group of cells whose proxies merge, and its merged proxy.
#[derive(Default)]
pub(super) struct Group {
    /// Its square on the grouping grid.
    pub bucket: (i64, i64),
    /// Member cells, by index.
    pub members: Vec<usize>,
    /// The members' proxy meshes it was (or is being) built from.
    built_from: Vec<Option<Handle<Mesh>>>,
    /// Where the merged mesh sits (the members' middle), following shifts.
    pub frame: Transform,
    /// Reach of the merged mesh from `frame`, meters.
    pub radius: f64,
    pub mesh: Option<Handle<Mesh>>,
    pub triangles: usize,
    building: Option<Mutex<Receiver<Option<Mesh>>>>,
    pub entity: Option<Entity>,
    /// Showing (or fading in); when the fade began.
    pub showing: bool,
    fade_started: Option<Instant>,
}

/// Every group, and the cells they were made from.
#[derive(Default)]
pub(super) struct Groups {
    pub list: Vec<Group>,
    /// Keys of the cells (and whether each takes part) the groups were made from.
    made_from: Vec<(String, bool)>,
    /// Per cell: a showing group stands in for its proxy.
    pub covered: Vec<bool>,
}

/// Distance at which a showing group gives way again, as a share of `group_from_m`.
const HIDE_SHARE: f64 = 0.85;

/// Makes, shows and hides the groups (before the cells' own proxies are shown, so a covered
/// cell's proxy stays down).
pub(super) fn update(
    world: &mut World,
    configs: &[ProxyConfig],
    sources: Option<&[(DVec3, f64)]>,
    settings: &HlodSettings,
) {
    regroup(world, configs, settings);
    let n = world.resource::<Proxies>().groups.list.len();
    let cells = world.resource::<StreamedCells>().cells.len();
    world
        .resource_mut::<Proxies>()
        .groups
        .covered
        .resize(cells, false);
    for g in 0..n {
        collect(world, g, settings);
        build(world, g, configs, settings);
        show(world, g, sources, settings);
    }
}

/// Puts each taking part cell in the group of its square, when the cells changed.
fn regroup(world: &mut World, configs: &[ProxyConfig], settings: &HlodSettings) {
    let cells = world.resource::<StreamedCells>();
    let proxies = world.resource::<Proxies>();
    let now: Vec<(String, bool)> = cells
        .cells
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let enabled = configs.get(i).copied().unwrap_or_default().enabled;
            // A cell takes part once its proxy has a place (the frame after it registers).
            let placed = proxies.slots.get(i).is_some_and(|s| s.frame.is_some());
            (
                c.key.clone(),
                enabled && placed && !c.retired && c.frame.is_none(),
            )
        })
        .collect();
    if now == proxies.groups.made_from {
        return;
    }
    let size = settings.group_m.max(1.0);
    let mut buckets: Vec<((i64, i64), Vec<usize>)> = Vec::new();
    {
        let proxies = world.resource::<Proxies>();
        for (i, (_, part)) in now.iter().enumerate() {
            let Some(frame) = proxies.slots.get(i).and_then(|s| s.frame).filter(|_| *part) else {
                continue;
            };
            let p = frame.translation;
            let bucket = ((p.x / size).floor() as i64, (p.z / size).floor() as i64);
            match buckets.iter_mut().find(|b| b.0 == bucket) {
                Some(b) => b.1.push(i),
                None => buckets.push((bucket, vec![i])),
            }
        }
    }
    // Old groups go, with their meshes; a lone cell has nothing to merge with.
    let old = std::mem::take(&mut world.resource_mut::<Proxies>().groups.list);
    for g in old {
        drop_group(world, g);
    }
    let list = buckets
        .into_iter()
        .filter(|b| b.1.len() >= 2)
        .map(|(bucket, members)| Group {
            bucket,
            members,
            ..Default::default()
        })
        .collect();
    let mut proxies = world.resource_mut::<Proxies>();
    proxies.groups.list = list;
    proxies.groups.made_from = now;
    proxies.groups.covered.clear();
}

fn drop_group(world: &mut World, g: Group) {
    if let Some(e) = g.entity
        && let Ok(entity) = world.get_entity_mut(e)
    {
        entity.despawn();
    }
    if let Some(mesh) = g.mesh
        && let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>()
    {
        meshes.remove(mesh);
    }
}

/// The members' proxy meshes now (`None` for one without a proxy yet).
fn member_meshes(world: &World, g: usize) -> Vec<Option<Handle<Mesh>>> {
    let proxies = world.resource::<Proxies>();
    proxies.groups.list[g]
        .members
        .iter()
        .map(|&i| proxies.slots.get(i).and_then(|s| s.mesh))
        .collect()
}

/// The group's disk name and version, when every member's proxy is kept on disk.
fn versioned(world: &World, g: usize, configs: &[ProxyConfig]) -> Option<(String, u64)> {
    let cells = world.resource::<StreamedCells>();
    let group = &world.resource::<Proxies>().groups.list[g];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &i in &group.members {
        let version = configs.get(i)?.version?;
        let key = &cells.cells.get(i)?.key;
        for b in key.bytes().chain(version.to_le_bytes()) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    let (x, z) = group.bucket;
    Some((format!("hlod1_{x}_{z}"), h))
}

/// Starts merging the members' proxies once every member that can have one has it (and again
/// when one of them changes), or takes the merged mesh from disk.
fn build(world: &mut World, g: usize, configs: &[ProxyConfig], settings: &HlodSettings) {
    let meshes = member_meshes(world, g);
    {
        let proxies = world.resource::<Proxies>();
        let group = &proxies.groups.list[g];
        if group.building.is_some() || group.built_from == meshes {
            return;
        }
        // Wait for every member's proxy (one found to have nothing to show counts as in).
        let waiting = group
            .members
            .iter()
            .zip(&meshes)
            .any(|(&i, m)| m.is_none() && !proxies.slots.get(i).is_some_and(|s| s.empty));
        if waiting || meshes.iter().flatten().count() < 2 {
            return;
        }
    }
    // The old merge goes; the members show on their own until the new one is in.
    let old = {
        let mut proxies = world.resource_mut::<Proxies>();
        let group = &mut proxies.groups.list[g];
        group.built_from = meshes.clone();
        group.triangles = 0;
        (group.mesh.take(), group.entity.take())
    };
    if let Some(e) = old.1
        && let Ok(entity) = world.get_entity_mut(e)
    {
        entity.despawn();
    }
    if let Some(mesh) = old.0 {
        world.resource_mut::<Assets<Mesh>>().remove(mesh);
    }
    hide(world, g);
    // Where it sits and how far it reaches.
    let (frames, members) = {
        let proxies = world.resource::<Proxies>();
        let group = &proxies.groups.list[g];
        let frames: Vec<Option<Transform>> = group
            .members
            .iter()
            .map(|&i| proxies.slots.get(i).and_then(|s| s.frame))
            .collect();
        (frames, group.members.clone())
    };
    let placed: Vec<(Transform, Handle<Mesh>)> = frames
        .iter()
        .zip(&meshes)
        .filter_map(|(f, m)| Some(((*f)?, (*m)?)))
        .collect();
    let center = placed
        .iter()
        .map(|(f, _)| f.translation)
        .fold(DVec3::ZERO, |a, b| a + b)
        / placed.len().max(1) as f64;
    let frame = Transform::from_translation(center);
    let radius = {
        let assets = world.resource::<Assets<Mesh>>();
        placed
            .iter()
            .map(|(f, m)| {
                let reach = assets.get(*m).map_or(0.0, |mesh| {
                    let b = mesh.aabb();
                    f64::from(b.min.abs().max(b.max.abs()).length())
                });
                f.translation.distance(center) + reach
            })
            .fold(1.0, f64::max)
    };
    {
        let mut proxies = world.resource_mut::<Proxies>();
        let group = &mut proxies.groups.list[g];
        group.frame = frame;
        group.radius = radius;
    }
    let disk = versioned(world, g, configs);
    if let Some((key, version)) = &disk
        && let Ok(mesh) = read_mesh(&cache_path(settings, key, *version))
    {
        store(world, g, mesh);
        return;
    }
    // Each member proxy as one source, placed in the group's space.
    let inverse = frame.rotation.inverse();
    let mut sources = Vec::new();
    let mut instances = Vec::new();
    {
        let assets = world.resource::<Assets<Mesh>>();
        for (f, m) in &placed {
            let Some(mesh) = assets.get(*m) else {
                continue;
            };
            sources.push(Source {
                positions: mesh
                    .vertices
                    .iter()
                    .map(|v| Vec3::from(v.position))
                    .collect(),
                colors: mesh
                    .vertices
                    .iter()
                    .map(|v| Vec3::new(v.color[0], v.color[1], v.color[2]))
                    .collect(),
                indices: mesh.indices.clone(),
            });
            instances.push(Instance {
                source: sources.len() - 1,
                matrix: Mat4::from_scale_rotation_translation(
                    f.scale,
                    inverse * f.rotation,
                    inverse * (f.translation - frame.translation).as_vec3(),
                ),
                color: Vec4::ONE,
            });
        }
    }
    let budget = settings.group_triangles;
    let label = format!("{} cells", members.len());
    let disk = disk.map(|(key, version)| cache_path(settings, &key, version));
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("hlod group".into())
        .spawn(move || {
            let t = Instant::now();
            let mesh = cluster(&sources, &instances, budget);
            if let Some((mesh, grid)) = &mesh {
                engine_core::log::info!(
                    "hlod: group of {label}: {} triangles (grid {grid:.2} m) in {:.2} s",
                    mesh.indices.len() / 3,
                    t.elapsed().as_secs_f32()
                );
            }
            let mesh = mesh.map(|(m, _)| m);
            if let (Some(mesh), Some(path)) = (&mesh, &disk)
                && let Err(e) = write_mesh(path, mesh)
            {
                engine_core::log::warn!("hlod: can't keep a group at {}: {e}", path.display());
            }
            let _ = tx.send(mesh);
        });
    if spawned.is_ok() {
        world.resource_mut::<Proxies>().groups.list[g].building = Some(Mutex::new(rx));
    }
}

/// Takes a finished merge.
fn collect(world: &mut World, g: usize, _settings: &HlodSettings) {
    let polled = {
        let mut proxies = world.resource_mut::<Proxies>();
        let Some(rx) = proxies.groups.list[g].building.as_mut() else {
            return;
        };
        match rx
            .get_mut()
            .map_or(Err(TryRecvError::Disconnected), |rx| rx.try_recv())
        {
            Ok(mesh) => mesh,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        }
    };
    world.resource_mut::<Proxies>().groups.list[g].building = None;
    if let Some(mesh) = polled {
        store(world, g, mesh);
    }
}

fn store(world: &mut World, g: usize, mesh: Mesh) {
    let triangles = mesh.indices.len() / 3;
    let handle = world.resource_mut::<Assets<Mesh>>().add(mesh);
    let group = &mut world.resource_mut::<Proxies>().groups.list[g];
    group.mesh = Some(handle);
    group.triangles = triangles;
}

/// Shows the group while every member is out and every source is far, cross-fading with the
/// members' own proxies; gives way to them otherwise.
fn show(world: &mut World, g: usize, sources: Option<&[(DVec3, f64)]>, settings: &HlodSettings) {
    let (members, mesh, frame, radius, showing) = {
        let group = &world.resource::<Proxies>().groups.list[g];
        (
            group.members.clone(),
            group.mesh,
            group.frame,
            group.radius,
            group.showing,
        )
    };
    let Some(mesh) = mesh else {
        return;
    };
    let all_out = {
        let cells = world.resource::<StreamedCells>();
        members.iter().all(|&i| {
            cells.cells.get(i).is_some_and(|c| {
                !matches!(
                    c.state,
                    CellState::Loaded | CellState::Loading | CellState::Unloading
                )
            })
        })
    };
    // Without sources (tours, tools) everything is in and nothing is far.
    let nearest = sources.map_or(0.0, |points| {
        points
            .iter()
            .map(|(p, scale)| (p.distance(frame.translation) - radius).max(0.0) / scale.max(1e-6))
            .fold(f64::INFINITY, f64::min)
    });
    let from = settings.group_from_m;
    let wanted = all_out
        && if showing {
            nearest >= from * HIDE_SHARE
        } else {
            nearest >= from
        };
    if wanted && !showing {
        let material = proxy_material(world);
        let entity = world
            .spawn((
                Name::new("hlod group proxy"),
                frame,
                GlobalTransform::from(frame),
                MeshRenderer {
                    mesh: MeshRef::Handle(mesh),
                    material: MaterialRef::Handle(material),
                    cast_shadows: true,
                    visible: true,
                },
                MaxDrawDistance(settings.group_draw_m),
                DitherFade::pair(0.0).1,
                HlodGroupProxy,
            ))
            .id();
        let group = &mut world.resource_mut::<Proxies>().groups.list[g];
        group.entity = Some(entity);
        group.showing = true;
        group.fade_started = Some(Instant::now());
    } else if !wanted && showing {
        hide(world, g);
        return;
    }
    if !wanted {
        return;
    }
    // Fading in over the members' proxies; then they go.
    let t = world.resource::<Proxies>().groups.list[g]
        .fade_started
        .map_or(1.0, |at| {
            at.elapsed().as_secs_f32() / settings.group_fade_s.max(1e-3)
        });
    let entity = world.resource::<Proxies>().groups.list[g].entity;
    let (out, into) = DitherFade::pair(t.min(1.0));
    if let Some(e) = entity
        && let Ok(mut e) = world.get_entity_mut(e)
    {
        if t >= 1.0 {
            e.remove::<DitherFade>();
        } else {
            e.insert(into);
        }
    }
    for &i in &members {
        let member = world
            .resource::<Proxies>()
            .slots
            .get(i)
            .and_then(|s| s.entity);
        if let Some(e) = member
            && let Ok(mut e) = world.get_entity_mut(e)
        {
            e.insert(out);
        }
        if t >= 1.0 {
            world.resource_mut::<Proxies>().groups.covered[i] = true;
        }
    }
    if t >= 1.0 {
        world.resource_mut::<Proxies>().groups.list[g].fade_started = None;
    }
}

/// The group gives way: its members' proxies are back at once and it goes.
fn hide(world: &mut World, g: usize) {
    let (members, entity) = {
        let mut proxies = world.resource_mut::<Proxies>();
        let group = &mut proxies.groups.list[g];
        group.showing = false;
        group.fade_started = None;
        (group.members.clone(), group.entity.take())
    };
    if let Some(e) = entity
        && let Ok(entity) = world.get_entity_mut(e)
    {
        entity.despawn();
    }
    for i in members {
        let member = {
            let mut proxies = world.resource_mut::<Proxies>();
            if let Some(c) = proxies.groups.covered.get_mut(i) {
                *c = false;
            }
            proxies.slots.get(i).and_then(|s| s.entity)
        };
        if let Some(e) = member
            && let Ok(mut e) = world.get_entity_mut(e)
        {
            e.remove::<DitherFade>();
        }
    }
}

/// On a group proxy's entity.
#[derive(Component, Debug, Clone, Copy)]
pub struct HlodGroupProxy;

impl Groups {
    pub(super) fn shift(&mut self, s: &WorldShift) {
        for g in &mut self.list {
            g.frame = s.transform(&g.frame);
        }
    }
}
