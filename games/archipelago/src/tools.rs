//! The hand on the map: what a click does (select, build, lay road, demolish), the ghost of
//! what is about to be built, keys, and what the bars' `isle.*` buttons do.

use std::collections::HashMap;
use std::sync::Arc;

use engine::input::Mouse;
use engine::prelude::*;
use engine::render::{InstanceRow, MeshInstances, Vertex};
use engine_ui::input::UiInputCapture;

use crate::art::Mat;
use crate::build;
use crate::camera;
use crate::defs::{CELL_M, Kind, Tier};
use crate::draw::Art;
use crate::grid::{self, Grid, Occ};
use crate::looks::{ModelKey, look};
use crate::sim::{Building, Economy, Producer};

/// What a left click does.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub enum Tool {
    #[default]
    Select,
    Build {
        kind: Kind,
        rot: u8,
    },
    Road,
    Demolish,
}

/// The player's hand between frames.
#[derive(Resource, Default)]
pub struct Hand {
    pub selected: Option<Entity>,
    /// Where a road drag began.
    road_from: Option<IVec2>,
    /// The right button's travel since it went down (a short one is a click: cancel).
    right_travel: f32,
    /// What the tool would do here, for the bar's hint line.
    pub hint: String,
}

/// The ghost's draw sets and the footprint tiles.
#[derive(Resource, Default)]
struct Ghost {
    sets: HashMap<(ModelKey, Mat), Entity>,
    tiles: Option<(Entity, Entity)>,
}

/// Ready the hand.
pub fn setup(world: &mut World) {
    world.init_resource::<Hand>();
    world.init_resource::<Ghost>();
}

fn set_tool(world: &mut World, tool: Tool) {
    *world.resource_mut::<Tool>() = tool;
    world.resource_mut::<Hand>().road_from = None;
    crate::hud::tool_changed(world);
}

fn select(world: &mut World, e: Option<Entity>) {
    world.resource_mut::<Hand>().selected = e;
    crate::hud::show_card(world);
}

fn toast(world: &mut World, title: &str, text: &str) {
    crate::hud::complain(world, title, text);
}

/// Where the cursor's cell is and the origin a `kind` turned `rot` would have centred on it.
fn origin_under(world: &mut World, kind: Kind, rot: u8) -> Option<(IVec2, IVec2)> {
    let hit = camera::pick(world)?;
    let c = grid::cell_of(hit.x, hit.z);
    let (w, d) = kind.footprint(rot);
    Some((c, c - IVec2::new(w / 2, d / 2)))
}

fn building_at(world: &World, c: IVec2) -> Option<Occ> {
    world.resource::<Grid>().cells.get(&c).copied()
}

/// Act on one of the bars' `isle.*` actions.
pub fn action(world: &mut World, a: &str) {
    if let Some(k) = a.strip_prefix("isle.build:").and_then(Kind::from_name) {
        let rot = match *world.resource::<Tool>() {
            Tool::Build { rot, .. } => rot,
            _ => 0,
        };
        set_tool(
            world,
            if k == Kind::Road {
                Tool::Road
            } else {
                Tool::Build { kind: k, rot }
            },
        );
    } else if a == "isle.tool:demolish" {
        set_tool(world, Tool::Demolish);
    } else if let Some(v) = a
        .strip_prefix("isle.speed:")
        .and_then(|s| s.parse::<f32>().ok())
    {
        world.resource_mut::<Economy>().speed = v;
    } else if a == "isle.deselect" {
        select(world, None);
    } else if a == "isle.sel.pause" {
        if let Some(e) = world.resource::<Hand>().selected
            && let Some(mut p) = world.get_mut::<Producer>(e)
        {
            p.paused = !p.paused;
        }
    } else if a == "isle.sel.demolish"
        && let Some(e) = world.resource::<Hand>().selected
    {
        build::demolish(world, e);
        select(world, None);
    }
}

/// Clicks and keys on the map.
pub fn input(world: &mut World) {
    if !world.contains_resource::<Hand>() {
        return;
    }
    let capture = world
        .get_resource::<UiInputCapture>()
        .cloned()
        .unwrap_or_default();
    let keys = world.resource::<ButtonInput<KeyCode>>().clone();
    let buttons = world.resource::<ButtonInput<MouseButton>>().clone();
    let delta = world.resource::<Mouse>().delta;
    let tool = *world.resource::<Tool>();
    if !capture.keyboard {
        if keys.just_pressed(KeyCode::R)
            && let Tool::Build { kind, rot } = tool
        {
            set_tool(
                world,
                Tool::Build {
                    kind,
                    rot: (rot + 1) % 4,
                },
            );
        }
        if keys.just_pressed(KeyCode::Escape) {
            if tool != Tool::Select {
                set_tool(world, Tool::Select);
            } else {
                select(world, None);
            }
        }
        if keys.just_pressed(KeyCode::Delete) {
            set_tool(world, Tool::Demolish);
        }
        for (k, v) in [
            (KeyCode::Digit1, 1.0),
            (KeyCode::Digit2, 2.0),
            (KeyCode::Digit3, 4.0),
        ] {
            if keys.just_pressed(k) {
                world.resource_mut::<Economy>().speed = v;
            }
        }
        if keys.just_pressed(KeyCode::Space) {
            let mut e = world.resource_mut::<Economy>();
            e.speed = if e.speed > 0.0 { 0.0 } else { 1.0 };
        }
    }
    // A short right click cancels the tool; a drag pans (camera.rs).
    if buttons.just_pressed(MouseButton::Right) {
        world.resource_mut::<Hand>().right_travel = 0.0;
    }
    if buttons.pressed(MouseButton::Right) {
        world.resource_mut::<Hand>().right_travel += delta.length();
    }
    if buttons.just_released(MouseButton::Right)
        && world.resource::<Hand>().right_travel < 6.0
        && !capture.pointer
    {
        set_tool(world, Tool::Select);
    }
    if capture.pointer {
        return;
    }
    let tool = *world.resource::<Tool>();
    match tool {
        Tool::Select => {
            if buttons.just_pressed(MouseButton::Left) {
                let hit = camera::pick(world).map(|h| grid::cell_of(h.x, h.z));
                let e = hit.and_then(|c| match building_at(world, c) {
                    Some(Occ::Building(e) | Occ::Field(e)) => Some(e),
                    _ => None,
                });
                select(world, e);
            }
        }
        Tool::Build { kind, rot } => {
            if buttons.just_pressed(MouseButton::Left)
                && let Some((_, origin)) = origin_under(world, kind, rot)
            {
                match build::place(world, kind, origin, rot, true) {
                    Ok(_) => {}
                    Err(msg) => toast(world, kind.def().name, &msg),
                }
            }
        }
        Tool::Road => {
            let cell = camera::pick(world).map(|h| grid::cell_of(h.x, h.z));
            if buttons.just_pressed(MouseButton::Left) {
                world.resource_mut::<Hand>().road_from = cell;
            }
            if buttons.just_released(MouseButton::Left)
                && let (Some(a), Some(b)) = (world.resource_mut::<Hand>().road_from.take(), cell)
                && let Err(msg) = build::place_road_line(world, a, b, true)
            {
                toast(world, "Road", &msg);
            }
        }
        Tool::Demolish => {
            if buttons.just_pressed(MouseButton::Left)
                && let Some(c) = camera::pick(world).map(|h| grid::cell_of(h.x, h.z))
            {
                match building_at(world, c) {
                    Some(Occ::Building(e)) => {
                        if world.resource::<Hand>().selected == Some(e) {
                            select(world, None);
                        }
                        build::demolish(world, e);
                    }
                    Some(Occ::Road) => build::remove_road(world, c),
                    Some(Occ::Field(_)) | None => {}
                }
            }
        }
    }
}

fn ghost_material(world: &mut World) -> Handle<StandardMaterial> {
    world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: [1.0, 1.0, 1.0, 0.42],
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            ..Default::default()
        })
}

fn tile_mesh() -> Mesh {
    let n = Vec3::Y;
    let v = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: n.into(),
        uv: [x + 0.5, z + 0.5],
        color: [1.0; 4],
    };
    Mesh::new(
        vec![v(-0.5, -0.5), v(0.5, -0.5), v(0.5, 0.5), v(-0.5, 0.5)],
        vec![0, 2, 1, 0, 3, 2],
    )
}

/// Cells the current tool would touch, and whether it can.
/// What a tool would do: the cells, whether it can, the building (kind, origin, turn) and a hint.
type Preview = (Vec<IVec2>, bool, Option<(Kind, IVec2, u8)>, String);

fn preview(world: &mut World, tool: Tool) -> Option<Preview> {
    match tool {
        Tool::Select => None,
        Tool::Build { kind, rot } => {
            let (_, origin) = origin_under(world, kind, rot)?;
            let ok = grid::check(world.resource::<Grid>(), kind, origin, rot);
            let msg = match ok {
                Ok(_) => format!(
                    "{}: click to build, R to turn, right click to stop.",
                    kind.def().name
                ),
                Err(b) => b.message().to_string(),
            };
            Some((
                grid::footprint(kind, origin, rot),
                ok.is_ok(),
                Some((kind, origin, rot)),
                msg,
            ))
        }
        Tool::Road => {
            let b = camera::pick(world).map(|h| grid::cell_of(h.x, h.z))?;
            let a = world.resource::<Hand>().road_from.unwrap_or(b);
            let mut cells = vec![a];
            let mut c = a;
            while c != b && cells.len() < 400 {
                if c.x != b.x {
                    c.x += (b.x - c.x).signum();
                } else {
                    c.y += (b.y - c.y).signum();
                }
                cells.push(c);
            }
            let g = world.resource::<Grid>();
            let ok = cells
                .iter()
                .all(|c| g.is_road(*c) || (g.is_open(*c) && grid::cell_roadable(*c)));
            let n = cells.iter().filter(|c| !g.is_road(**c)).count();
            let msg = format!(
                "Road: drag to lay {n} cells for {} coins.",
                n * Kind::Road.def().coins as usize
            );
            Some((cells, ok, None, msg))
        }
        Tool::Demolish => {
            let c = camera::pick(world).map(|h| grid::cell_of(h.x, h.z))?;
            let cells = match building_at(world, c) {
                Some(Occ::Building(e)) => world
                    .get::<Building>(e)
                    .map(|b| grid::footprint(b.kind, b.origin, b.rot))
                    .unwrap_or_default(),
                Some(Occ::Road) => vec![c],
                Some(Occ::Field(_)) | None => vec![],
            };
            Some((
                cells,
                false,
                None,
                "Demolish: click a building or a road.".into(),
            ))
        }
    }
}

/// Draw the ghost of what the tool would do.
pub fn ghost(world: &mut World) {
    if !world.contains_resource::<Ghost>()
        || world.resource::<EngineConfig>().headless && world.resource::<Tool>() == &Tool::Select
    {
        return;
    }
    let Some(art) = world.get_resource::<Art>().cloned() else {
        return;
    };
    if world.resource::<Ghost>().tiles.is_none() {
        let mesh = world.resource_mut::<Assets<Mesh>>().add(tile_mesh());
        let mat = ghost_material(world);
        let mut r = MeshRenderer::new(mesh, mat);
        r.cast_shadows = false;
        let ok = world
            .spawn((
                Name::new("Ghost tiles"),
                Transform::default(),
                r.clone(),
                MeshInstances::new(Vec::new()),
            ))
            .id();
        let bad = world
            .spawn((
                Name::new("Ghost tiles (blocked)"),
                Transform::default(),
                r,
                MeshInstances::new(Vec::new()),
            ))
            .id();
        world.resource_mut::<Ghost>().tiles = Some((ok, bad));
    }
    let tool = *world.resource::<Tool>();
    let p = preview(world, tool);
    let mut tiles: Vec<InstanceRow> = Vec::new();
    let mut groups: HashMap<(ModelKey, Mat), Vec<InstanceRow>> = HashMap::new();
    let mut hint = String::new();
    let mut ok_all = true;
    if let Some((cells, ok, building, msg)) = p {
        ok_all = ok;
        hint = msg;
        let tint = if tool == Tool::Demolish {
            [1.0, 0.25, 0.2]
        } else if ok {
            [0.35, 1.0, 0.45]
        } else {
            [1.0, 0.3, 0.25]
        };
        for c in &cells {
            let p = grid::cell_center(*c);
            let y = grid::cell_range(*c).1.max(0.0) + 0.12;
            tiles.push(InstanceRow {
                position: Vec3::new(p.x as f32, y as f32, p.y as f32),
                yaw: 0.0,
                scale: CELL_M as f32 * 0.94,
                tint,
            });
        }
        if let Some((kind, origin, rot)) = building {
            let c = grid::footprint_center(kind, origin, rot);
            let base = Vec3::new(
                c.x as f32,
                grid::base_height(kind, origin, rot) as f32,
                c.y as f32,
            );
            let yaw = grid::yaw(rot);
            let q = Quat::from_rotation_y(yaw);
            for piece in look(kind, Tier::Fisherfolk, 7) {
                let Some(model) = art.models.get(piece.model) else {
                    continue;
                };
                for (_, mat) in model.iter() {
                    groups
                        .entry((piece.model, *mat))
                        .or_default()
                        .push(InstanceRow {
                            position: base + q * piece.pos,
                            yaw: yaw + piece.yaw,
                            scale: piece.scale,
                            tint: piece.tint,
                        });
                }
            }
        }
    }
    let (ok_e, bad_e) = world.resource::<Ghost>().tiles.unwrap();
    let (a, b) = if ok_all && tool != Tool::Demolish {
        (tiles, Vec::new())
    } else {
        (Vec::new(), tiles)
    };
    if let Some(mut i) = world.get_mut::<MeshInstances>(ok_e) {
        *i = MeshInstances::new(a);
    }
    if let Some(mut i) = world.get_mut::<MeshInstances>(bad_e) {
        *i = MeshInstances::new(b);
    }
    let mut ghost = std::mem::take(&mut *world.resource_mut::<Ghost>());
    for (key, e) in &ghost.sets {
        if !groups.contains_key(key)
            && let Some(mut inst) = world.get_mut::<MeshInstances>(*e)
        {
            inst.rows = Arc::from(Vec::new());
        }
    }
    for (key, rows) in groups {
        if let Some(&e) = ghost.sets.get(&key) {
            if let Some(mut inst) = world.get_mut::<MeshInstances>(e) {
                *inst = MeshInstances::new(rows);
            }
            continue;
        }
        let Some((mesh, mat)) = art.models[key.0].iter().find(|(_, m)| *m == key.1) else {
            continue;
        };
        let mut r = MeshRenderer::new(*mesh, art.mats.get(*mat));
        r.cast_shadows = false;
        let e = world
            .spawn((
                Name::new("Ghost"),
                Transform::default(),
                r,
                MeshInstances::new(rows),
            ))
            .id();
        ghost.sets.insert(key, e);
    }
    *world.resource_mut::<Ghost>() = ghost;
    world.resource_mut::<Hand>().hint = hint;
}
