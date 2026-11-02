//! `SHOWCASE_CENSUS=<file.md>`: at every tour stop, count what the scene holds and what the
//! frame draws, the way Unreal's `stat scenerendering` and World Partition's streaming view list
//! it per cell, and append it to `<file.md>` (a table row per stop, then the stop's details).
//!
//! Counted per stop: entities (all, and per loaded cell), mesh entities and their raw triangles
//! (in view, casting shadows within the shadow distance), what the renderer really drew (draws,
//! instances, triangles after its levels of detail, clusters and culling, per feature), the
//! heaviest groups of meshes and the heaviest single meshes with their owners, plants still
//! growing, and the slowest systems when `--profile` is on. "In view" is a cone a bit wider than
//! the camera's, measured to each mesh's bounding sphere; raw triangles are before the
//! renderer's automatic levels of detail, so the drawn count is the one the GPU pays.

use std::collections::HashMap;
use std::fmt::Write as _;

use engine::core::diagnostics::Diagnostics;
use engine::core::profile::ProfileSnapshot;
use engine::prelude::*;
use engine::render::{MaxDrawDistance, Mesh, MeshRef, NoMeshLod, ShadowOnly, with_renderer};
use engine::world::stream::CellInfo;
use engine::world::{CellMember, CellState, StreamedCells};

use crate::scenery::Lod;

/// Shadow maps cover this far (Rendering's `shadow_distance` at Ultra), meters.
const SHADOW_M: f64 = 350.0;
/// Components every mesh has, which say nothing about whose it is.
const PLUMBING: &[&str] = &[
    "Transform",
    "GlobalTransform",
    "MeshRenderer",
    "ChildOf",
    "Children",
    "Lod",
    "LodFade",
    "DitherFade",
    "MaxDrawDistance",
    "NoMeshLod",
    "NoVirtualGeometry",
    "ShadowOnly",
    "CellMember",
    "FollowsWorldShift",
    "HideWithin",
    "TransformTreeChanged",
    "Disabled",
];
/// Rows in the heaviest-groups and heaviest-meshes lists.
const TOP: usize = 12;

/// The file the census appends to, when on.
pub fn path() -> Option<String> {
    std::env::var("SHOWCASE_CENSUS")
        .ok()
        .filter(|p| !p.is_empty() && p != "0")
}

/// `SHOWCASE_CENSUS_BLIND=1` with the census: count without drawing (no screenshots and no
/// drawn counts), for machines without a GPU.
pub fn blind() -> bool {
    path().is_some() && std::env::var("SHOWCASE_CENSUS_BLIND").is_ok_and(|v| v == "1")
}

#[derive(Default, Clone, Copy)]
struct Tally {
    count: u32,
    in_view: u32,
    tris_view: u64,
    tris_cast: u64,
}

impl Tally {
    fn add(&mut self, tris: u64, view: bool, cast: bool) {
        self.count += 1;
        if view {
            self.in_view += 1;
            self.tris_view += tris;
        }
        if cast {
            self.tris_cast += tris;
        }
    }
}

/// Count the stop and append it to the census file.
pub fn take(world: &mut World, stop: &str, first: bool) {
    let Some(path) = path() else {
        return;
    };
    let text = census(world, stop, first);
    use std::io::Write as _;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path);
    match file {
        Ok(mut f) => {
            let _ = f.write_all(text.as_bytes());
        }
        Err(e) => engine::core::log::warn!("census: can't write {path}: {e}"),
    }
    for line in text.lines().take(3) {
        engine::core::log::info!("census: {line}");
    }
}

fn census(world: &mut World, stop: &str, first: bool) -> String {
    // The eye and its cone.
    let mut cams = world.query::<(&Camera, &GlobalTransform)>();
    let Some((cam, eye)) = cams
        .iter(world)
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(c, t)| (*c, *t))
    else {
        return format!("| {stop} | no camera |\n");
    };
    let fov_y = match cam.projection {
        Projection::Perspective { fov_y_deg, .. } => fov_y_deg,
        _ => 65.0,
    };
    // Half the diagonal field at 16:9, plus a margin.
    let half = ((fov_y.to_radians() * 0.5).tan() * (1.0f32 + (16.0f32 / 9.0).powi(2)).sqrt()).atan()
        as f64
        + 0.1;
    let (eye_p, fwd) = (eye.translation(), eye.forward().as_dvec3());

    // Cells.
    let cells = world
        .get_resource::<StreamedCells>()
        .cloned()
        .unwrap_or_default();
    let cell_key = |id: u32| {
        cells
            .cells
            .get(id as usize)
            .map_or_else(|| format!("cell {id}"), |c| c.key.clone())
    };

    // Who owns each entity: the nearest Name up the hierarchy, its cell, and the root.
    let mut names: HashMap<Entity, String> = HashMap::new();
    let mut q = world.query::<(Entity, &Name)>();
    for (e, n) in q.iter(world) {
        names.insert(e, n.as_str().to_string());
    }
    let mut parents: HashMap<Entity, Entity> = HashMap::new();
    let mut q = world.query::<(Entity, &ChildOf)>();
    for (e, c) in q.iter(world) {
        parents.insert(e, c.parent());
    }
    let mut members: HashMap<Entity, u32> = HashMap::new();
    let mut q = world.query::<(Entity, &CellMember)>();
    for (e, c) in q.iter(world) {
        members.insert(e, c.0.0);
    }
    // Unnamed meshes are told apart by their most telling component (the archetype's first
    // one that isn't plumbing every mesh has), as the editor's outliner shows them.
    let mut kinds: HashMap<u32, Option<String>> = HashMap::new();
    let mut kind_of = |w: &World, e: Entity| -> Option<String> {
        let arch = w.entities().get_spawned(e).ok()?.archetype_id;
        kinds
            .entry(arch.index() as u32)
            .or_insert_with(|| {
                w.inspect_entity(e).ok()?.find_map(|(_, info)| {
                    let full = info.name().to_string();
                    let base = full.split('<').next().unwrap_or(&full);
                    let short = base.rsplit("::").next().unwrap_or(base);
                    let krate = base.split("::").next().unwrap_or("");
                    (!PLUMBING.contains(&short)).then(|| format!("{krate}::{short}"))
                })
            })
            .clone()
    };
    let mut owner = |w: &World, mut e: Entity| -> (Option<String>, Option<u32>) {
        let (mut name, mut kind, mut cell) = (None, None, None);
        for _ in 0..32 {
            if name.is_none() {
                name = names.get(&e).cloned();
            }
            if name.is_none() && kind.is_none() {
                kind = kind_of(w, e);
            }
            if cell.is_none() {
                cell = members.get(&e).copied();
            }
            match parents.get(&e) {
                Some(p) => e = *p,
                None => break,
            }
        }
        (name.or(kind), cell)
    };

    let mut q = world.query::<(
        Entity,
        &MeshRenderer,
        &GlobalTransform,
        Option<&MaxDrawDistance>,
        Has<ShadowOnly>,
        Has<Lod>,
        Has<NoMeshLod>,
    )>();
    let rows: Vec<_> = q
        .iter(world)
        .map(|(e, r, t, max, shadow_only, lod, no_lod)| {
            (
                e,
                r.clone(),
                *t,
                max.map_or(f32::MAX, |m| m.0),
                shadow_only,
                lod,
                no_lod,
            )
        })
        .collect();
    // Meshes.
    let meshes = world.resource::<Assets<Mesh>>();
    let mut info: HashMap<Handle<Mesh>, (u64, Vec3, f32)> = HashMap::new();
    let mut mesh_info = |h: Handle<Mesh>| {
        *info.entry(h).or_insert_with(|| {
            meshes.get(h).map_or((0, Vec3::ZERO, 0.0), |m| {
                let b = m.aabb();
                let c = (b.min + b.max) * 0.5;
                (
                    (m.indices.len() / 3) as u64,
                    c,
                    (b.max - b.min).length() * 0.5,
                )
            })
        })
    };
    let mut total = Tally::default();
    let mut by_group: HashMap<String, Tally> = HashMap::new();
    let mut by_mesh: HashMap<Handle<Mesh>, (Tally, u64, String, bool)> = HashMap::new();
    let mut no_cell = Tally::default();
    let (mut hidden, mut lods, mut shapes) = (0u32, 0u32, 0u32);
    for (e, r, t, max, shadow_only, lod, no_lod) in rows {
        lods += lod as u32;
        if !r.visible {
            hidden += 1;
            continue;
        }
        let MeshRef::Handle(h) = r.mesh else {
            shapes += 1;
            continue;
        };
        let (tris, c, radius) = mesh_info(h);
        let scale = t.scale().max_element() as f64;
        let center = t.transform_point(c.as_dvec3());
        let radius = radius as f64 * scale;
        let to = center - eye_p;
        let dist = (to.length() - radius).max(0.0);
        if dist > max as f64 {
            continue;
        }
        let ahead = to.length() <= radius || {
            let angle = to.normalize().dot(fwd).clamp(-1.0, 1.0).acos();
            angle - (radius / to.length()).min(1.0).asin() <= half
        };
        let view = ahead && !shadow_only;
        let cast = r.cast_shadows && dist <= SHADOW_M;
        let (name, cell) = owner(world, e);
        let group = format!(
            "{} / {}{}",
            cell.map_or_else(|| "always".to_string(), cell_key),
            name.clone().unwrap_or_else(|| "(unnamed)".into()),
            if lod { " [Lod]" } else { "" }
        );
        total.add(tris, view, cast);
        if cell.is_none() {
            no_cell.add(tris, view, cast);
        }
        by_group
            .entry(group.clone())
            .or_default()
            .add(tris, view, cast);
        let m = by_mesh
            .entry(h)
            .or_insert_with(|| (Tally::default(), tris, group, no_lod));
        m.0.add(tris, view, cast);
    }

    // What the renderer drew.
    let d = world.get_resource::<Diagnostics>();
    let get = |k: &str| d.and_then(|d| d.get(k)).unwrap_or(f64::NAN);
    let (draws, instances, drawn) = (
        get("render.draw_calls"),
        get("render.instances"),
        get("render.triangles"),
    );
    let features = with_renderer(world, |r, _| r.describe()).unwrap_or_default();

    // Plants.
    let mut q = world.query::<&engine_foliage::plant::Plant>();
    let (mut plants, mut growing) = (0u32, 0u32);
    for p in q.iter(world) {
        plants += 1;
        growing += (p.stage.is_alive() && p.size < 0.999) as u32;
    }

    let entities = world.iter_entities().count();
    let loaded: Vec<&CellInfo> = cells
        .cells
        .iter()
        .filter(|c| !c.retired && c.state == CellState::Loaded)
        .collect();
    let m = |x: u64| x as f64 / 1e6;

    let mut out = String::new();
    if first {
        let _ = writeln!(
            out,
            "\n| Stop | Entities | Cells loaded | Mesh entities (in view) | Raw tris in view | Raw tris casting | Drawn tris | Draws | Instances | Plants (growing) |\n|---|---|---|---|---|---|---|---|---|---|"
        );
    }
    let _ = writeln!(
        out,
        "| {stop} | {entities} | {} | {} ({}) | {:.1} M | {:.1} M | {:.1} M | {draws:.0} | {instances:.0} | {plants} ({growing}) |",
        loaded.len(),
        total.count,
        total.in_view,
        m(total.tris_view),
        m(total.tris_cast),
        drawn / 1e6,
    );

    // Details, under the table (a later run's table rows still read fine with them between).
    let mut more = String::new();
    let _ = writeln!(more, "\n<details><summary>{stop}</summary>\n");
    let _ = writeln!(
        more,
        "- Mesh entities: {} drawn, {hidden} hidden, {shapes} built-in shapes, {lods} with the showcase's `Lod`. Outside any cell (always loaded): {} meshes, {:.1} M raw tris in view, {:.1} M casting.",
        total.count,
        no_cell.count,
        m(no_cell.tris_view),
        m(no_cell.tris_cast)
    );
    let mut cells_line: Vec<_> = loaded
        .iter()
        .map(|c| format!("{} ({} ent, {:.2} s)", c.key, c.entities, c.load_secs))
        .collect();
    cells_line.sort();
    let _ = writeln!(more, "- Cells loaded: {}", cells_line.join(", "));
    if first && let Some(table) = crate::spawn_costs::table(world) {
        let _ = writeln!(more, "\nZone spawn slices (main thread):\n\n{table}");
    }
    if let Some(list) = features["features"].as_array() {
        let mut list: Vec<_> = list
            .iter()
            .filter(|f| f["draw_calls"].as_u64().unwrap_or(0) > 0)
            .collect();
        list.sort_by_key(|f| std::cmp::Reverse(f["triangles"].as_u64().unwrap_or(0)));
        let line: Vec<_> = list
            .iter()
            .take(10)
            .map(|f| {
                format!(
                    "{} {} draws {:.2} M",
                    f["name"].as_str().unwrap_or("?"),
                    f["draw_calls"],
                    f["triangles"].as_u64().unwrap_or(0) as f64 / 1e6
                )
            })
            .collect();
        let _ = writeln!(more, "- Drawn by feature: {}", line.join("; "));
        if let Some(mf) = list.iter().find(|f| f["name"] == "meshes") {
            let s = &mf["state"];
            let _ = writeln!(
                more,
                "- Mesh batches: {} opaque, {} transparent, shadow {}, far shadow {}, impostors in view {}",
                s["opaque_batches"],
                s["transparent_batches"],
                s["shadow_batches"],
                s["far_shadow_batches"],
                s["impostors_in_view"]
            );
        }
    }
    let mut groups: Vec<_> = by_group.into_iter().collect();
    groups.sort_by_key(|(_, t)| std::cmp::Reverse(t.tris_view.max(t.tris_cast)));
    let _ = writeln!(
        more,
        "\n| Heaviest groups (cell / owner) | Meshes | In view | Raw tris in view | Raw tris casting |\n|---|---|---|---|---|"
    );
    for (g, t) in groups.iter().take(TOP) {
        let _ = writeln!(
            more,
            "| {g} | {} | {} | {:.2} M | {:.2} M |",
            t.count,
            t.in_view,
            m(t.tris_view),
            m(t.tris_cast)
        );
    }
    let mut heavy: Vec<_> = by_mesh.into_iter().collect();
    heavy.sort_by_key(|(_, (t, ..))| std::cmp::Reverse(t.tris_view.max(t.tris_cast)));
    let _ = writeln!(
        more,
        "\n| Heaviest meshes (first owner) | Tris each | Copies | In view | Raw tris casting | Auto LOD |\n|---|---|---|---|---|---|"
    );
    for (h, (t, each, g, no_lod)) in heavy.iter().take(TOP) {
        let _ = writeln!(
            more,
            "| {g} (mesh {}) | {each} | {} | {} | {:.2} M | {} |",
            h.index(),
            t.count,
            t.in_view,
            m(t.tris_cast),
            if *no_lod { "off" } else { "on" }
        );
    }
    if let Some(p) = world.get_resource::<ProfileSnapshot>()
        && !p.systems.is_empty()
    {
        let top: Vec<_> = p
            .systems
            .iter()
            .take(8)
            .map(|(s, ms)| format!("{s} {ms:.2}"))
            .collect();
        let _ = writeln!(
            more,
            "\n- Systems (ms a frame, all threads {:.1} of {:.1}): {}",
            p.busy_ms,
            p.frame_ms,
            top.join("; ")
        );
    }
    let _ = writeln!(more, "\n</details>");
    // Keep the table rows together: the details go to a second file next to the table.
    if let Some(path) = path() {
        use std::io::Write as _;
        let details = format!("{}.details.md", path.trim_end_matches(".md"));
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(details)
        {
            let _ = f.write_all(more.as_bytes());
        }
    }
    out
}
