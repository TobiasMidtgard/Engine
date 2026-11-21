//! Blueprints (his note 23): buildings and roads laid as plans. A blueprint costs nothing,
//! does nothing and keeps its ground until the player has it built: a click on it with no
//! tool in hand, or Shift+N for every one the company can pay for (roads first, then the
//! buildings in the order they were laid). Nothing is built on its own.
//!
//! N switches blueprint mode on and off: in it, every building and road the hand lays is a
//! blueprint. Out of it, a building the company can't pay for yet is laid as a blueprint
//! instead, and the hint says so before the click. Blueprints show as pale blue silhouettes
//! over blue cells, red where something has since been built on their ground; X clears one.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

use engine::prelude::*;
use engine::render::Vertex;

use crate::art::Mat;
use crate::build;
use crate::defs::{CELL_M, Kind, Tier};
use crate::draw::{Art, MapRow, Tiled};
use crate::grid::{self, Grid};
use crate::looks::{ModelKey, look};

/// A building laid as a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Print {
    pub kind: Kind,
    pub origin: IVec2,
    pub rot: u8,
    pub isle: usize,
}

impl Print {
    pub fn cells(&self) -> Vec<IVec2> {
        grid::footprint(self.kind, self.origin, self.rot)
    }
}

/// The company's blueprints, and blueprint mode.
#[derive(Resource, Default)]
pub struct Blueprints {
    /// Blueprint mode: the hand lays plans instead of building.
    pub on: bool,
    /// Planned buildings, oldest first.
    pub prints: Vec<Print>,
    /// Planned road cells, oldest first, and whether each is to be paved.
    pub roads: Vec<(IVec2, bool)>,
    sets: Tiled<(ModelKey, Mat)>,
    marks: Tiled<u8>,
    looks: Option<Looks>,
    made_for: Option<u64>,
}

#[derive(Clone)]
struct Looks {
    glass: Handle<StandardMaterial>,
    tile: MeshRenderer,
}

/// The blue the plans are drawn in (brighter than white light, so it reads from afar).
pub const BLUE: [f32; 3] = [0.42, 0.72, 1.6];
const RED: [f32; 3] = [1.5, 0.32, 0.26];
/// The silhouettes' tint over the glass.
const GLASS_TINT: [f32; 3] = [0.75, 1.05, 1.8];

pub fn setup(world: &mut World) {
    world.init_resource::<Blueprints>();
}

/// Every cell a planned building keeps.
pub fn kept(world: &World) -> HashSet<IVec2> {
    world
        .get_resource::<Blueprints>()
        .map(|b| b.prints.iter().flat_map(|p| p.cells()).collect())
        .unwrap_or_default()
}

/// The planned building over cell `c`.
pub fn print_at(world: &World, c: IVec2) -> Option<Print> {
    world
        .get_resource::<Blueprints>()?
        .prints
        .iter()
        .find(|p| p.cells().contains(&c))
        .copied()
}

/// Whether a road is planned on cell `c`.
pub fn road_at(world: &World, c: IVec2) -> bool {
    world
        .get_resource::<Blueprints>()
        .is_some_and(|b| b.roads.iter().any(|r| r.0 == c))
}

pub fn mode(world: &World) -> bool {
    world.get_resource::<Blueprints>().is_some_and(|b| b.on)
}

/// Whether `kind` may be built at `origin` turned `rot` as far as the blueprints go: their
/// ground is theirs, unless it is the very blueprint (building it).
pub fn clear_for(world: &World, kind: Kind, origin: IVec2, rot: u8) -> Result<(), String> {
    let Some(bp) = world.get_resource::<Blueprints>() else {
        return Ok(());
    };
    let me = Print {
        kind,
        origin,
        rot,
        isle: 0,
    };
    let cells = grid::footprint(kind, origin, rot);
    for p in &bp.prints {
        if (p.kind, p.origin, p.rot) == (me.kind, me.origin, me.rot) {
            continue;
        }
        if p.cells().iter().any(|c| cells.contains(c)) {
            return Err(format!(
                "A blueprint of a {} keeps that ground: click it with no tool in hand to build \
                 it, or clear it with X.",
                p.kind.def().name.to_lowercase()
            ));
        }
    }
    if bp.roads.iter().any(|(c, _)| cells.contains(c)) {
        return Err("A planned road runs there: build it (Shift+N) or clear it with X.".into());
    }
    Ok(())
}

/// Whether `kind` turned `rot` may be laid as a blueprint at `origin`, and on which island:
/// the ground must take it, the company must hold the island, and no other plan may be there.
pub fn check(world: &World, kind: Kind, origin: IVec2, rot: u8) -> Result<usize, String> {
    let isle = grid::check(world.resource::<Grid>(), kind, origin, rot)
        .map_err(|b| b.message().to_string())?;
    if !crate::settle::claimed(world, isle) {
        return Err(crate::settle::unclaimed_text_in(world, isle));
    }
    if world.get_resource::<Blueprints>().is_some_and(|b| {
        b.prints
            .iter()
            .any(|p| (p.kind, p.origin, p.rot) == (kind, origin, rot))
    }) {
        return Err("That blueprint is laid already.".into());
    }
    clear_for(world, kind, origin, rot)?;
    Ok(isle)
}

/// Lay `kind` as a blueprint.
pub fn lay(world: &mut World, kind: Kind, origin: IVec2, rot: u8) -> Result<(), String> {
    let isle = check(world, kind, origin, rot)?;
    world.resource_mut::<Blueprints>().prints.push(Print {
        kind,
        origin,
        rot,
        isle,
    });
    Ok(())
}

/// Plan a road over `cells` (those not a road of that kind already, on the company's own
/// islands and off planned buildings): how many cells were planned.
pub fn lay_road(world: &mut World, cells: &[IVec2], paved: bool) -> usize {
    let kept = kept(world);
    let new: Vec<IVec2> = {
        let g = world.resource::<Grid>();
        cells
            .iter()
            .copied()
            .filter(|c| !kept.contains(c))
            .filter(|c| {
                if paved {
                    !g.is_paved(*c)
                } else {
                    !g.is_road(*c)
                }
            })
            .filter(|c| {
                let at = grid::cell_center(*c);
                crate::island::island_at(at.x, at.y)
                    .is_some_and(|i| crate::settle::claimed(world, i))
            })
            .collect()
    };
    let mut bp = world.resource_mut::<Blueprints>();
    for c in &new {
        match bp.roads.iter_mut().find(|r| r.0 == *c) {
            Some(r) => r.1 |= paved,
            None => bp.roads.push((*c, paved)),
        }
    }
    new.len()
}

/// Forget the blueprint of exactly this building (it has been built), if there is one.
pub fn forget(world: &mut World, kind: Kind, origin: IVec2, rot: u8) {
    if let Some(mut bp) = world.get_resource_mut::<Blueprints>() {
        bp.prints
            .retain(|p| (p.kind, p.origin, p.rot) != (kind, origin, rot));
    }
}

/// Clear the blueprint over cell `c` (a building's or a road's): whether there was one.
pub fn clear_at(world: &mut World, c: IVec2) -> bool {
    let Some(mut bp) = world.get_resource_mut::<Blueprints>() else {
        return false;
    };
    let n = bp.prints.len() + bp.roads.len();
    bp.prints.retain(|p| !p.cells().contains(&c));
    bp.roads.retain(|r| r.0 != c);
    bp.prints.len() + bp.roads.len() < n
}

/// Whether `kind` may be put up and paid for now: open (see unlocks) and affordable.
/// What may not is laid as a blueprint and waits for it.
pub fn ready(world: &World, kind: Kind, isle: usize) -> Result<(), String> {
    match crate::unlocks::refusal(world, kind) {
        Some(why) => Err(why),
        None => build::can_pay(world, kind, isle),
    }
}

/// Build the planned building `p` now, if the company can pay.
pub fn build_print(world: &mut World, p: Print) -> Result<Entity, String> {
    let e = build::place(world, p.kind, p.origin, p.rot, true)?;
    forget(world, p.kind, p.origin, p.rot);
    Ok(e)
}

/// Build one planned road cell. A cell that has become a road of its kind meanwhile is
/// simply dropped.
fn build_road_cell(world: &mut World, c: IVec2, paved: bool) -> Result<(), String> {
    let done = {
        let g = world.resource::<Grid>();
        if paved { g.is_paved(c) } else { g.is_road(c) }
    };
    if !done {
        if paved {
            build::place_paving(world, c, true)?;
        } else {
            build::place_road(world, c, true)?;
        }
    }
    world
        .resource_mut::<Blueprints>()
        .roads
        .retain(|r| r.0 != c);
    Ok(())
}

/// Build the planned road through cell `c` (every planned cell joined to it).
pub fn build_road_at(world: &mut World, c: IVec2) -> Result<usize, String> {
    let planned: HashMap<IVec2, bool> = world
        .resource::<Blueprints>()
        .roads
        .iter()
        .copied()
        .collect();
    let mut seen = HashSet::from([c]);
    let mut todo = VecDeque::from([c]);
    let mut order = Vec::new();
    while let Some(x) = todo.pop_front() {
        if !planned.contains_key(&x) {
            continue;
        }
        order.push(x);
        for r in 0..4 {
            let n = x + grid::front_dir(r);
            if seen.insert(n) {
                todo.push_back(n);
            }
        }
    }
    let mut built = 0;
    for x in order {
        build_road_cell(world, x, planned[&x])?;
        built += 1;
    }
    Ok(built)
}

/// What [`build_all`] did.
#[derive(Debug, Default)]
pub struct Built {
    pub built: usize,
    pub waiting: usize,
    /// Why the first that waits does.
    pub why: Option<String>,
}

/// Build every blueprint the company can pay for: the roads first, then the buildings in the
/// order they were laid, passing over those it can't pay for (or whose ground is taken).
pub fn build_all(world: &mut World) -> Built {
    let mut out = Built::default();
    let roads = world.resource::<Blueprints>().roads.clone();
    for (c, paved) in roads {
        match build_road_cell(world, c, paved) {
            Ok(()) => out.built += 1,
            Err(e) => {
                out.why.get_or_insert(e);
            }
        }
    }
    let prints = world.resource::<Blueprints>().prints.clone();
    for p in prints {
        match build_print(world, p) {
            Ok(_) => out.built += 1,
            Err(e) => {
                out.why.get_or_insert(e);
            }
        }
    }
    let bp = world.resource::<Blueprints>();
    out.waiting = bp.prints.len() + bp.roads.len();
    out
}

/// The cost of `kind` as a line: "250 coins, 4 planks".
pub fn cost_text(kind: Kind) -> String {
    let def = kind.def();
    let mut parts = vec![format!("{} coins", def.coins)];
    for (g, n) in def.cost {
        parts.push(format!("{n} {}", g.name().to_lowercase()));
    }
    parts.join(", ")
}

fn quad() -> Mesh {
    let v = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: [0.0, 1.0, 0.0],
        uv: [x + 0.5, z + 0.5],
        color: [1.0; 4],
    };
    Mesh::new(
        vec![v(-0.5, -0.5), v(0.5, -0.5), v(0.5, 0.5), v(-0.5, 0.5)],
        vec![0, 2, 1, 0, 3, 2],
    )
}

fn looks(world: &mut World) -> Looks {
    if let Some(l) = world.resource::<Blueprints>().looks.clone() {
        return l;
    }
    let glass = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: [0.5, 0.72, 1.0, 0.42],
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            ..Default::default()
        });
    let film = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: [1.0, 1.0, 1.0, 0.5],
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            ..Default::default()
        });
    let mesh = world.resource_mut::<Assets<Mesh>>().add(quad());
    let mut tile = MeshRenderer::new(mesh, film);
    tile.cast_shadows = false;
    let l = Looks { glass, tile };
    world.resource_mut::<Blueprints>().looks = Some(l.clone());
    l
}

/// The blueprints' glass, for the hand's ghost in blueprint mode.
pub fn glass(world: &mut World) -> Handle<StandardMaterial> {
    looks(world).glass
}

/// A renderer for one material of a model drawn in the blueprints' `glass`.
pub fn glass_renderer(
    art: &Art,
    key: (ModelKey, Mat),
    glass: &Handle<StandardMaterial>,
) -> Option<MeshRenderer> {
    let (mesh, _) = art.models.get(key.0)?.iter().find(|(_, m)| *m == key.1)?;
    let mut r = MeshRenderer::new(*mesh, *glass);
    r.cast_shadows = false;
    Some(r)
}

pub fn glass_tint() -> [f32; 3] {
    GLASS_TINT
}

/// Draw the blueprints: pale blue silhouettes over blue cells (red where the ground has been
/// taken since), and planned roads as blue cells.
pub fn draw(world: &mut World) {
    if !world.contains_resource::<Blueprints>() {
        return;
    }
    let Some(art) = world.get_resource::<Art>().cloned() else {
        return;
    };
    let lift = crate::placing::lift(world);
    let key = {
        let bp = world.resource::<Blueprints>();
        let g = world.resource::<Grid>();
        let mut s = std::collections::hash_map::DefaultHasher::new();
        bp.prints.hash(&mut s);
        bp.roads.hash(&mut s);
        (g.cells.len(), g.roads.len(), g.paved.len()).hash(&mut s);
        ((lift * 4.0) as i64).hash(&mut s);
        s.finish()
    };
    if world.resource::<Blueprints>().made_for == Some(key) {
        return;
    }
    let l = looks(world);
    let (prints, roads) = {
        let bp = world.resource::<Blueprints>();
        (bp.prints.clone(), bp.roads.clone())
    };
    let mut groups: HashMap<(ModelKey, Mat), Vec<MapRow>> = HashMap::new();
    let mut tiles: HashMap<u8, Vec<MapRow>> = HashMap::new();
    let tile = |c: IVec2, scale: f32, tint: [f32; 3]| {
        let at = grid::cell_center(c);
        MapRow {
            at: DVec3::new(at.x, grid::cell_range(c).1.max(0.0) + 0.12 + lift, at.y),
            yaw: 0.0,
            scale,
            tint,
        }
    };
    {
        let g = world.resource::<Grid>();
        for p in &prints {
            let ok = grid::check(g, p.kind, p.origin, p.rot).is_ok();
            for c in p.cells() {
                tiles.entry(0).or_default().push(tile(
                    c,
                    CELL_M as f32 * 0.92,
                    if ok { BLUE } else { RED },
                ));
            }
            let c = grid::footprint_center(p.kind, p.origin, p.rot);
            let base = DVec3::new(c.x, grid::base_height(p.kind, p.origin, p.rot), c.y);
            let yaw = grid::yaw(p.rot);
            let q = Quat::from_rotation_y(yaw);
            let biome = crate::island::islands()[p.isle].biome();
            for piece in look(p.kind, Tier::Fisherfolk, 7, biome) {
                let Some(model) = art.models.get(piece.model) else {
                    continue;
                };
                for (_, mat) in model.iter() {
                    groups.entry((piece.model, *mat)).or_default().push(MapRow {
                        at: base + (q * piece.pos).as_dvec3(),
                        yaw: yaw + piece.yaw,
                        scale: piece.scale,
                        tint: GLASS_TINT,
                    });
                }
            }
        }
        for (c, paved) in &roads {
            let ok = g.is_open(*c) || g.is_road(*c);
            let tint = match (ok, paved) {
                (false, _) => RED,
                (true, true) => [0.75, 0.86, 1.7],
                (true, false) => BLUE,
            };
            tiles
                .entry(0)
                .or_default()
                .push(tile(*c, CELL_M as f32 * 0.7, tint));
        }
    }
    let mut bp = std::mem::take(&mut *world.resource_mut::<Blueprints>());
    let film = l.tile.clone();
    bp.marks.set(world, tiles, |_| Some(film.clone()));
    let glass = l.glass;
    bp.sets
        .set(world, groups, |key| glass_renderer(&art, key, &glass));
    bp.made_for = Some(key);
    *world.resource_mut::<Blueprints>() = bp;
}

/// (For the tour: a row of houses, a market and the road before them laid as blueprints
/// from `near` on, wherever they fit.)
pub(crate) fn tour_lay(world: &mut World, near: IVec2) -> usize {
    let (w, d) = Kind::House.footprint(0);
    let mut n = 0;
    for k in 0..6 {
        let o = near + IVec2::new(k * w, 0);
        if lay(world, Kind::House, o, 0).is_ok() {
            n += 1;
        }
    }
    let road: Vec<IVec2> = (-1..6 * w + 1)
        .map(|x| near + IVec2::new(x, -1))
        .filter(|c| {
            let g = world.resource::<Grid>();
            g.is_open(*c) && grid::cell_roadable(*c)
        })
        .collect();
    lay_road(world, &road, false);
    let (mw, _) = Kind::Market.footprint(0);
    for dx in [6 * w + 1, -mw - 1] {
        if lay(
            world,
            Kind::Market,
            near + IVec2::new(dx, d - Kind::Market.footprint(0).1),
            0,
        )
        .is_ok()
        {
            n += 1;
            break;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_town() -> World {
        let mut w = crate::town::tests::bare_world();
        crate::town::build_start(&mut w);
        w.init_resource::<Blueprints>();
        w
    }

    /// Open ground on the home island that takes a house.
    fn spot(w: &World) -> IVec2 {
        let home = crate::island::home_town_spot();
        let c0 = grid::cell_of(home.x, home.y);
        (0..200i32)
            .flat_map(|r| {
                (-r..=r)
                    .flat_map(move |x| (-r..=r).map(move |z| IVec2::new(x, z)))
                    .filter(move |o| o.x.abs().max(o.y.abs()) == r)
            })
            .map(|o| c0 + o)
            .find(|c| (0..3).all(|k| check(w, Kind::House, *c + IVec2::new(k * 3, 0), 0).is_ok()))
            .expect("open ground near the town")
    }

    #[test]
    fn blueprints_cost_nothing_keep_their_ground_and_build_when_paid() {
        let mut w = world_with_town();
        let o = spot(&w);
        let me = crate::sim::acting(&w);
        let before = w.resource::<crate::sim::Economy>().coins_of(me);
        lay(&mut w, Kind::House, o, 0).unwrap();
        assert_eq!(
            w.resource::<crate::sim::Economy>().coins_of(me),
            before,
            "a blueprint costs nothing"
        );
        // Its ground is kept from anything else, but not from itself.
        assert!(check(&w, Kind::House, o, 0).is_err(), "laid twice");
        assert!(clear_for(&w, Kind::Market, o - IVec2::new(1, 1), 0).is_err());
        assert!(clear_for(&w, Kind::House, o, 0).is_ok());
        assert!(
            crate::placing::road_path_around(
                w.resource::<Grid>(),
                &kept(&w),
                o + IVec2::new(1, 1),
                o + IVec2::new(1, 6)
            )
            .is_none_or(|p| p
                .iter()
                .all(|c| !grid::footprint(Kind::House, o, 0).contains(c)))
        );
        // With no money, it waits; with money, Shift+N builds it.
        *w.resource_mut::<crate::sim::Economy>().coins_mut(me) = -1.0;
        let r = build_all(&mut w);
        assert_eq!((r.built, r.waiting), (0, 1), "{r:?}");
        assert!(r.why.is_some());
        *w.resource_mut::<crate::sim::Economy>().coins_mut(me) = 10_000.0;
        let r = build_all(&mut w);
        assert_eq!((r.built, r.waiting), (1, 0), "{r:?}");
        assert!(matches!(
            w.resource::<Grid>().cells.get(&o),
            Some(grid::Occ::Building(_))
        ));
        assert!(w.resource::<Blueprints>().prints.is_empty());
    }

    #[test]
    fn a_planned_road_is_built_as_one_and_cleared_cell_by_cell() {
        let mut w = world_with_town();
        let o = spot(&w);
        let cells: Vec<IVec2> = (0..5).map(|x| o + IVec2::new(x, 0)).collect();
        assert_eq!(lay_road(&mut w, &cells, false), 5);
        assert!(clear_at(&mut w, cells[4]));
        assert_eq!(w.resource::<Blueprints>().roads.len(), 4);
        let me = crate::sim::acting(&w);
        *w.resource_mut::<crate::sim::Economy>().coins_mut(me) = 10_000.0;
        assert_eq!(build_road_at(&mut w, cells[1]), Ok(4));
        assert!(cells[..4].iter().all(|c| w.resource::<Grid>().is_road(*c)));
        assert!(w.resource::<Blueprints>().roads.is_empty());
    }
}
