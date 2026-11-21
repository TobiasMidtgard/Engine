//! Commanding the fleet like a real-time strategy game: click a ship to pick her, drag a box
//! round several, Shift adds and takes away, a double click picks every ship of that kind in
//! view. Ctrl and a number makes a control group, the number alone picks it again and a
//! second press brings the view there. A right click on the sea sends the picked craft there
//! in a loose formation (a ship on a trade route leaves it), and the card bottom left gives
//! them their orders: Scout (F) charts the fog by itself, Stop (Backspace) heaves to, Home
//! (B) puts back to the nearest harbour. Period picks the next craft lying idle.
//!
//! Picked craft wear a green ring on the water, their course runs ahead of them as a dashed
//! line (amber for a scout) and a ring marks where they are bound; an order flashes where it
//! was given. Everything is drawn into one small mesh rebuilt each frame.
//!
//! The rules (what an order does to a vessel) are here and in `settle::sail_to`; the input
//! hooks sit in `tools::input`.

use engine::input::Mouse;
use engine::prelude::*;
use engine::render::{Mesh, NoMeshLod, NoVirtualGeometry, Projection, Vertex};
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::Screen;
use engine_ui::style::{Align, Direction, Fill, Pick, Position, Val};

use crate::camera::StrategyCam;
use crate::defs::Kind;
use crate::fleet::{Doing, Roam, Routes, Vessel};
use crate::fog::{self, FogOfWar};
use crate::globe::{self, Frame};
use crate::island;
use crate::sim::Building;
use crate::vessels::Craft;

pub const SCREEN: &str = "isle_helm";
const BOX_ID: &str = "isle.helm.box";
const CARD_ID: &str = "isle.helm.card";
/// A press that travels further than this, physical px, is a drag (a box).
const DRAG_PX: f32 = 7.0;
/// Two presses closer than this, real seconds, are a double click (or a double tap).
const DOUBLE_S: f32 = 0.4;
/// Ships in a formation keep this far apart, meters.
const SPACING_M: f64 = 120.0;
/// At most this many craft listed on the card; the rest are counted.
const LISTED: usize = 7;

const GREEN: [f32; 3] = [0.38, 1.0, 0.48];
const AMBER: [f32; 3] = [1.0, 0.74, 0.25];
const CREAM: [f32; 3] = [0.95, 0.9, 0.75];
const GOLD: [f32; 3] = [1.0, 0.82, 0.3];

/// The picked craft, the control groups and the box being drawn.
#[derive(Resource, Default)]
pub struct Helm {
    pub selected: Vec<Entity>,
    /// Control groups 1..9 (index 0 is the 0 key).
    pub groups: [Vec<Entity>; 10],
    /// Where the left button went down (physical px), while it is held.
    press: Option<Vec2>,
    boxing: bool,
    /// The press picked a craft (so its release doesn't clear the pick).
    pressed_ship: bool,
    last_click: Option<(Entity, f32)>,
    last_recall: Option<(usize, f32)>,
    hover: Option<Entity>,
    pings: Vec<Ping>,
    overlay: Option<(Entity, Handle<Mesh>)>,
    material: Option<Handle<StandardMaterial>>,
    /// What the card last showed (rebuilt only when it changes).
    sig: String,
    /// Real seconds since the start.
    clock: f32,
}

/// The flash where an order was given.
struct Ping {
    at: DVec3,
    age: f32,
    color: [f32; 3],
}

/// Ready the helm: its resource and its screen (the box and the card).
pub fn setup(world: &mut World) {
    world.init_resource::<Helm>();
    if world.get_resource::<engine_ui::theme::Theme>().is_none() {
        return;
    }
    let screen = Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 2,
        closable: false,
        ..Default::default()
    };
    let el = El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(El::new().id(BOX_ID).with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .top(0.0)
                .width(0.0)
                .height(0.0)
                .opacity(0.0)
                .background(Fill::Solid(engine_ui::Color::hexa(0x5cff7a26)))
                .border(2.0, engine_ui::Color::hexa(0x7dff95ee))
                .radius(3.0)
                .pick(Pick::Ignore)
        }))
        .child(El::new().id(CARD_ID).with_style(|s| {
            s.position(Position::Absolute)
                .left(22.0)
                .bottom(22.0)
                .direction(Direction::Column)
                .pick(Pick::Ignore)
        }));
    if let Err(e) = engine_ui::screens::spawn_screen(world, screen, &el) {
        engine::core::log::warn!("archipelago helm: {e}");
    }
}

// ------------------------------------------------------------------------------ the view

/// What the camera sees, for putting map things on the screen.
struct View {
    t: Transform,
    /// tan of half the vertical field of view.
    th: f32,
    w: f32,
    h: f32,
    dist: f32,
}

fn view(world: &mut World) -> Option<View> {
    let window = world.resource::<engine::core::window::Window>().clone();
    let mut q = world.query::<(&Transform, &Camera, &StrategyCam)>();
    let (t, cam, sc) = q.iter(world).next()?;
    let fov = match cam.projection {
        Projection::Perspective { fov_y_deg, .. } => fov_y_deg.to_radians(),
        _ => 65f32.to_radians(),
    };
    Some(View {
        t: *t,
        th: (fov * 0.5).tan(),
        w: window.width.max(1) as f32,
        h: window.height.max(1) as f32,
        dist: sc.dist,
    })
}

impl View {
    /// A world point on the screen (physical px), and its depth; `None` behind the camera.
    fn project(&self, p: DVec3) -> Option<(Vec2, f32)> {
        let local = self.t.rotation.inverse() * (p - self.t.translation).as_vec3();
        let depth = -local.z;
        if depth < 1.0 {
            return None;
        }
        let x = local.x / depth / (self.th * self.w / self.h);
        let y = local.y / depth / self.th;
        Some((
            Vec2::new((x + 1.0) * 0.5 * self.w, (1.0 - y) * 0.5 * self.h),
            depth,
        ))
    }

    /// Pixels a meter spans at `depth`.
    fn px_per_m(&self, depth: f32) -> f32 {
        self.h * 0.5 / (self.th * depth)
    }
}

/// The player's craft on the screen: entity, where, and how far round her a click still
/// counts (px).
fn on_screen(world: &mut World, v: &View) -> Vec<(Entity, Vec2, f32)> {
    let mut q = world.query::<(Entity, &Vessel, &Transform)>();
    q.iter(world)
        .filter(|(_, x, _)| x.commanded())
        .filter_map(|(e, x, t)| {
            let (s, depth) = v.project(t.translation)?;
            let r = (x.craft.half_length() * 1.3 * v.px_per_m(depth)).max(20.0);
            Some((e, s, r))
        })
        .filter(|(_, s, _)| s.x >= 0.0 && s.y >= 0.0 && s.x <= v.w && s.y <= v.h)
        .collect()
}

/// The craft of the player's under screen point `px`, nearest first.
fn ship_under(world: &mut World, px: Vec2) -> Option<Entity> {
    let v = view(world)?;
    on_screen(world, &v)
        .into_iter()
        .filter(|(_, s, r)| (*s - px).length() <= *r)
        .min_by(|a, b| (a.1 - px).length().total_cmp(&(b.1 - px).length()))
        .map(|x| x.0)
}

fn shift(world: &World) -> bool {
    let k = world.resource::<ButtonInput<KeyCode>>();
    k.pressed(KeyCode::ShiftLeft) || k.pressed(KeyCode::ShiftRight)
}

fn ctrl(world: &World) -> bool {
    let k = world.resource::<ButtonInput<KeyCode>>();
    k.pressed(KeyCode::ControlLeft) || k.pressed(KeyCode::ControlRight)
}

// ------------------------------------------------------------------------------ picking

/// Pick these craft (Shift: add them, or take them away if all are picked already).
fn pick(world: &mut World, list: Vec<Entity>, add: bool) {
    let mut h = world.resource_mut::<Helm>();
    if add {
        if !list.is_empty() && list.iter().all(|e| h.selected.contains(e)) {
            h.selected.retain(|e| !list.contains(e));
        } else {
            for e in list {
                if !h.selected.contains(&e) {
                    h.selected.push(e);
                }
            }
        }
    } else {
        h.selected = list;
    }
    let any = !h.selected.is_empty();
    if any {
        // One thing in the hand at a time: the building card closes.
        crate::tools::select(world, None);
    }
}

/// Clear the pick.
pub fn clear(world: &mut World) {
    if let Some(mut h) = world.get_resource_mut::<Helm>() {
        h.selected.clear();
    }
}

/// The left button with the Select tool, from `tools::input`: returns true when the press
/// picked a craft (the building under it is not picked then).
pub fn input(world: &mut World, buttons: &ButtonInput<MouseButton>) -> bool {
    if !world.contains_resource::<Helm>() {
        return false;
    }
    let Some(px) = world.resource::<Mouse>().position else {
        return false;
    };
    let mut took = false;
    if buttons.just_pressed(MouseButton::Left) {
        let now = world.resource::<Helm>().clock;
        let hit = ship_under(world, px);
        {
            let mut h = world.resource_mut::<Helm>();
            h.press = Some(px);
            h.boxing = false;
            h.pressed_ship = hit.is_some();
        }
        if let Some(e) = hit {
            let double = world
                .resource::<Helm>()
                .last_click
                .is_some_and(|(x, t)| x == e && now - t < DOUBLE_S);
            if double {
                // Every craft of her kind in view.
                let craft = world.get::<Vessel>(e).map(|v| v.craft);
                let all: Vec<Entity> = match view(world) {
                    Some(v) => on_screen(world, &v)
                        .into_iter()
                        .map(|x| x.0)
                        .filter(|x| world.get::<Vessel>(*x).map(|v| v.craft) == craft)
                        .collect(),
                    None => vec![e],
                };
                let add = shift(world);
                pick(world, all, add);
            } else {
                let add = shift(world);
                pick(world, vec![e], add);
            }
            world.resource_mut::<Helm>().last_click = Some((e, now));
            took = true;
        }
    }
    if buttons.pressed(MouseButton::Left)
        && let Some(from) = world.resource::<Helm>().press
        && (px - from).length() > DRAG_PX
    {
        world.resource_mut::<Helm>().boxing = true;
    }
    if buttons.just_released(MouseButton::Left) {
        let (from, boxing, on_ship) = {
            let mut h = world.resource_mut::<Helm>();
            let r = (h.press.take(), h.boxing, h.pressed_ship);
            h.boxing = false;
            r
        };
        if boxing && let Some(from) = from {
            let (lo, hi) = (from.min(px), from.max(px));
            let inside: Vec<Entity> = match view(world) {
                Some(v) => on_screen(world, &v)
                    .into_iter()
                    .filter(|(_, s, _)| s.x >= lo.x && s.y >= lo.y && s.x <= hi.x && s.y <= hi.y)
                    .map(|x| x.0)
                    .collect(),
                None => Vec::new(),
            };
            let add = shift(world);
            if !inside.is_empty() || !add {
                pick(world, inside, add);
            }
        } else if !on_ship && !shift(world) {
            clear(world);
        }
    }
    took
}

/// A short right click with the Select tool, from `tools::input`: send the picked craft to
/// the point under the cursor. Returns false when nothing is picked (the click cancels as
/// before).
pub fn right_click(world: &mut World) -> bool {
    if world
        .get_resource::<Helm>()
        .is_none_or(|h| h.selected.is_empty())
    {
        return false;
    }
    if *world.resource::<crate::tools::Tool>() != crate::tools::Tool::Select {
        return false;
    }
    let Some(hit) = crate::camera::pick(world) else {
        return true;
    };
    let list = world.resource::<Helm>().selected.clone();
    send(world, &list, DVec2::new(hit.x, hit.z));
    true
}

// ------------------------------------------------------------------------------ orders

/// Where a click at `p` sends a ship: on the sea, there; on an island, off the company's
/// harbour there, or off its landfall.
fn sea_point(world: &mut World, p: DVec2) -> (DVec2, String) {
    let Some(i) = island::island_at(p.x, p.y) else {
        return (p, waters(p));
    };
    let isle = &island::islands()[i];
    let me = crate::sim::PLAYER;
    let econ = world.resource::<crate::sim::Economy>().clone();
    let mut q = world.query::<&Building>();
    if let Some(b) = q
        .iter(world)
        .find(|b| b.island == i && b.kind == Kind::Harbour && econ.owns(me, i))
    {
        return (crate::fleet::mooring(b).0, isle.name.to_string());
    }
    (
        island::settle_coast(isle).unwrap_or(isle.center),
        isle.name.to_string(),
    )
}

/// The island a point at sea is off, if it is near one; else open water.
fn waters(p: DVec2) -> String {
    let i = island::nearest_island(p.x, p.y);
    let isle = &island::islands()[i];
    if (p - isle.center).length() < isle.radius * 2.5 {
        format!("the waters off {}", isle.name)
    } else {
        "open water".into()
    }
}

/// The `k`th place of a loose formation round its middle: the middle, then rings of six,
/// twelve, eighteen.
fn formation(k: usize) -> DVec2 {
    if k == 0 {
        return DVec2::ZERO;
    }
    let (mut ring, mut first) = (1usize, 1usize);
    while k >= first + ring * 6 {
        first += ring * 6;
        ring += 1;
    }
    let n = ring * 6;
    let a = (k - first) as f64 / n as f64 * std::f64::consts::TAU + ring as f64 * 0.4;
    DVec2::new(a.cos(), a.sin()) * SPACING_M * ring as f64
}

/// Free a craft for an order of the player's: off her route, out of her circling.
fn stand_by(world: &mut World, e: Entity) -> Result<(), String> {
    let on_route = world.get::<Vessel>(e).is_some_and(|v| v.route.is_some());
    if on_route {
        crate::orders::unassign(world, e)?;
    }
    let mut v = world.get_mut::<Vessel>(e).ok_or("no such vessel")?;
    if !v.commanded() {
        return Err(format!("{} is not one of the company's.", v.name));
    }
    if matches!(v.roam, Roam::Circling { .. } | Roam::Scout { .. }) {
        let at = v.at;
        v.roam = Roam::Idle { at };
    }
    Ok(())
}

/// A loop of points round `c` at `y` meters, for an aircraft to circle.
fn circle(c: DVec2, y: f64, r: f64) -> std::sync::Arc<Vec<DVec3>> {
    std::sync::Arc::new(
        (0..8)
            .map(|i| {
                let a = i as f64 / 8.0 * std::f64::consts::TAU;
                DVec3::new(c.x + a.cos() * r, y, c.y + a.sin() * r)
            })
            .collect(),
    )
}

/// Send one craft to `to` (an aircraft circles there).
pub fn send_one(world: &mut World, e: Entity, to: DVec2) -> Result<(), String> {
    stand_by(world, e)?;
    let flies = world.get::<Vessel>(e).is_some_and(|v| v.craft.flies());
    if flies {
        let mut v = world.get_mut::<Vessel>(e).ok_or("no such vessel")?;
        let y = v.altitude as f64;
        v.roam = Roam::Circling {
            points: circle(to, y, 320.0),
        };
        v.next_stop = 0;
        v.doing = Doing::Lingering(0.0);
        Ok(())
    } else {
        crate::settle::sail_to(world, e, to)
    }
}

/// Send craft `list` to map point `p`, in formation, and say so.
fn send(world: &mut World, list: &[Entity], p: DVec2) {
    let (to, place) = sea_point(world, p);
    let mut sent = Vec::new();
    let mut why = None;
    let mut left_routes = 0;
    // Ships nearest the spot take the middle.
    let mut order: Vec<(Entity, f64)> = list
        .iter()
        .filter_map(|e| {
            let v = world.get::<Vessel>(*e)?;
            Some((*e, (DVec2::new(v.at.x, v.at.z) - to).length()))
        })
        .collect();
    order.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (k, (e, _)) in order.into_iter().enumerate() {
        let was_on_route = world.get::<Vessel>(e).is_some_and(|v| v.route.is_some());
        match send_one(world, e, to + formation(k)) {
            Ok(()) => {
                if was_on_route {
                    left_routes += 1;
                }
                if let Some(v) = world.get::<Vessel>(e) {
                    sent.push(v.name.clone());
                }
            }
            Err(e) => why = Some(e),
        }
    }
    let y = 0.0;
    world.resource_mut::<Helm>().pings.push(Ping {
        at: DVec3::new(to.x, y, to.y),
        age: 0.0,
        color: GREEN,
    });
    let title = match sent.len() {
        0 => "They stay".to_string(),
        1 => format!("{} sails", sent[0]),
        n => format!("{n} craft sail"),
    };
    let mut text = if sent.is_empty() {
        why.unwrap_or_default()
    } else {
        format!("Bound for {place}.")
    };
    if left_routes > 0 {
        text.push_str(&format!(
            " {left_routes} left {} trade route{} for it.",
            if left_routes == 1 { "her" } else { "their" },
            if left_routes == 1 { "" } else { "s" }
        ));
    }
    tell(world, &title, &text);
}

/// The company's harbour (ships) or airship mast (aircraft) nearest `p`, as a place to lie.
fn home_for(world: &mut World, p: DVec3, flies: bool) -> Option<DVec3> {
    let econ = world.resource::<crate::sim::Economy>().clone();
    let me = crate::sim::PLAYER;
    let kind = if flies {
        Kind::AirshipMast
    } else {
        Kind::Harbour
    };
    let mut q = world.query::<&Building>();
    q.iter(world)
        .filter(|b| b.kind == kind && econ.owns(me, b.island))
        .map(|b| {
            if flies {
                crate::fleet::mast_point(b)
            } else {
                let m = crate::fleet::mooring(b).0;
                DVec3::new(m.x, 0.0, m.y)
            }
        })
        .min_by(|a, b| globe::distance(*a, p).total_cmp(&globe::distance(*b, p)))
}

/// Put craft `e` to charting the fog.
pub fn scout(world: &mut World, e: Entity) -> Result<(), String> {
    stand_by(world, e)?;
    let (at, flies) = {
        let v = world.get::<Vessel>(e).ok_or("no such vessel")?;
        (v.at, v.craft.flies())
    };
    let home = home_for(world, at, flies).unwrap_or(at);
    let mut v = world.get_mut::<Vessel>(e).ok_or("no such vessel")?;
    v.roam = Roam::Scout { home };
    v.doing = Doing::Lingering(0.0);
    Ok(())
}

/// Heave to where she is (an aircraft circles there).
fn stop(world: &mut World, e: Entity) -> Result<(), String> {
    let at = world.get::<Vessel>(e).ok_or("no such vessel")?.at;
    send_one(world, e, DVec2::new(at.x, at.z))
}

/// Back to the nearest harbour (or mast).
fn go_home(world: &mut World, e: Entity) -> Result<(), String> {
    let (at, flies) = {
        let v = world.get::<Vessel>(e).ok_or("no such vessel")?;
        (v.at, v.craft.flies())
    };
    let Some(home) = home_for(world, at, flies) else {
        return Err("The company has no harbour to put back to yet.".into());
    };
    stand_by(world, e)?;
    if flies {
        // To the mast itself: an idle aircraft noses up to it.
        let mut v = world.get_mut::<Vessel>(e).ok_or("no such vessel")?;
        v.roam = Roam::Idle { at: home };
        v.doing = Doing::Lingering(0.0);
        Ok(())
    } else {
        crate::settle::sail_to(world, e, DVec2::new(home.x, home.z))
    }
}

/// Give every picked craft one of the card's orders.
fn order_all(world: &mut World, what: &str) {
    let list = world.resource::<Helm>().selected.clone();
    if list.is_empty() {
        return;
    }
    let mut done = 0;
    let mut why = None;
    for e in &list {
        let r = match what {
            "scout" => scout(world, *e),
            "stop" => stop(world, *e),
            _ => go_home(world, *e),
        };
        match r {
            Ok(()) => done += 1,
            Err(e) => why = Some(e),
        }
    }
    let n = |k: usize| {
        if k == 1 {
            "1 craft".to_string()
        } else {
            format!("{k} craft")
        }
    };
    let (title, text) = match what {
        "scout" => (
            format!("{} scouting", n(done)),
            "They chart the nearest fog, then the next, and put back to harbour when no unseen \
             sea is left in reach."
                .to_string(),
        ),
        "stop" => (
            format!("{} heave to", n(done)),
            "They wait for orders.".to_string(),
        ),
        _ => (
            format!("{} put back", n(done)),
            "Bound for the nearest harbour of yours.".to_string(),
        ),
    };
    if done == 0 {
        tell(world, "No orders taken", &why.unwrap_or_default());
    } else {
        tell(world, &title, &text);
    }
}

// ------------------------------------------------------------------------------ scouting

/// The next stretch of fog for a scout, or the way home when no fog is left in reach.
/// Called when she sets out on a leg (`fleet::depart`).
pub fn scout_leg(world: &mut World, v: &mut Vessel) -> Option<DVec3> {
    let Roam::Scout { home } = v.roam.clone() else {
        return None;
    };
    if let Some(p) = scout_goal(world, v) {
        return Some(p);
    }
    v.roam = Roam::Idle { at: home };
    if v.company == crate::sim::PLAYER {
        tell(
            world,
            &format!("{} has charted her waters", v.name),
            "No unseen sea is left in her reach. She is putting back to harbour.",
        );
    }
    Some(home)
}

/// Where a scout sails next: the nearest unseen sea she can reach, a little ahead of her
/// rather than behind, and away from where the other scouts are bound.
pub fn scout_goal(world: &mut World, v: &Vessel) -> Option<DVec3> {
    let here = DVec2::new(v.at.x, v.at.z);
    let flies = v.craft.flies();
    let sight = fog::sight_m(v);
    let others: Vec<DVec2> = {
        let mut q = world.query::<&Vessel>();
        q.iter(world)
            .filter(|x| x.name != v.name && matches!(x.roam, Roam::Scout { .. }))
            .filter_map(|x| x.path.last().map(|p| DVec2::new(p.x, p.z)))
            .collect()
    };
    let mut found = {
        let fog = world.get_resource::<FogOfWar>()?;
        candidates(fog, here, v.heading, sight, flies, &others)
    };
    found.truncate(8);
    for p in found {
        if flies {
            return Some(DVec3::new(p.x, v.altitude as f64, p.y));
        }
        let reachable = world
            .get_resource_mut::<crate::fleet::SeaChart>()
            .is_none_or(|mut c| c.voyage(here, p).is_some());
        if reachable {
            return Some(DVec3::new(p.x, 0.0, p.y));
        }
    }
    None
}

/// Unseen points a scout at `here` could make for, best first.
fn candidates(
    fog: &FogOfWar,
    here: DVec2,
    heading: f32,
    sight: f64,
    flies: bool,
    others: &[DVec2],
) -> Vec<DVec2> {
    let region = globe::region_of(here.x, here.y);
    let origin = globe::sheet_origin(region);
    let reach = globe::regions()[region].reach_m * 0.9;
    let fwd = DVec2::new(heading.cos() as f64, -(heading.sin() as f64));
    let mut out: Vec<(f64, DVec2)> = Vec::new();
    for k in 1..=24 {
        let r = sight * 0.8 * k as f64;
        let n = (16 + k * 4).min(64);
        for i in 0..n {
            let a = i as f64 / n as f64 * std::f64::consts::TAU + k as f64 * 0.37;
            let dir = DVec2::new(a.cos(), a.sin());
            let p = here + dir * r;
            if (p - origin).length() > reach {
                continue;
            }
            if !flies && island::height(p.x, p.y) > -6.0 {
                continue;
            }
            if fog.seen_at(globe::dir(p.x, p.y)) > 0.3 {
                continue;
            }
            if others.iter().any(|o| (*o - p).length() < sight * 1.6) {
                continue;
            }
            let ahead = dir.dot(fwd);
            out.push((r * (1.0 + 0.45 * (1.0 - ahead)), p));
        }
        // The nearest ring with fog in it is far enough to look at.
        if out.len() >= 12 {
            break;
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|x| x.1).collect()
}

// ------------------------------------------------------------------------------ keys and card

/// `isle.helm.*` buttons. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    let Some(rest) = a.strip_prefix("isle.helm.") else {
        return false;
    };
    if !world.contains_resource::<Helm>() {
        return true;
    }
    match rest {
        "scout" | "stop" | "home" => order_all(world, rest),
        "clear" => clear(world),
        _ => {
            if let Some(i) = rest
                .strip_prefix("only:")
                .and_then(|s| s.parse::<usize>().ok())
            {
                let pick_one = world.resource::<Helm>().selected.get(i).copied();
                if let Some(e) = pick_one {
                    pick(world, vec![e], false);
                    look_at(world, &[e]);
                }
            } else if let Some(g) = rest
                .strip_prefix("group:")
                .and_then(|s| s.parse::<usize>().ok())
            {
                recall(world, g.min(9));
            }
        }
    }
    true
}

/// Glide the view to the middle of these craft.
fn look_at(world: &mut World, list: &[Entity]) {
    let pts: Vec<DVec3> = list
        .iter()
        .filter_map(|e| world.get::<Vessel>(*e).map(|v| v.at))
        .collect();
    if pts.is_empty() {
        return;
    }
    let mid = pts.iter().fold(DVec3::ZERO, |a, p| a + *p) / pts.len() as f64;
    let y = island::height(mid.x, mid.z).max(0.0);
    let mut q = world.query::<&mut StrategyCam>();
    for mut cam in q.iter_mut(world) {
        let far = globe::region_of(cam.focus.x, cam.focus.z) != globe::region_of(mid.x, mid.z);
        cam.goal_focus = DVec3::new(mid.x, y, mid.z);
        if far {
            cam.focus = cam.goal_focus;
        }
    }
}

/// Pick control group `g`; a second press soon after brings the view there.
fn recall(world: &mut World, g: usize) {
    let now = world.resource::<Helm>().clock;
    let list = world.resource::<Helm>().groups[g].clone();
    if list.is_empty() {
        return;
    }
    let again = world
        .resource::<Helm>()
        .last_recall
        .is_some_and(|(x, t)| x == g && now - t < DOUBLE_S);
    pick(world, list.clone(), false);
    if again {
        look_at(world, &list);
    }
    world.resource_mut::<Helm>().last_recall = Some((g, now));
}

fn digit(k: &ButtonInput<KeyCode>) -> Option<usize> {
    [
        KeyCode::Digit0,
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ]
    .into_iter()
    .position(|d| k.just_pressed(d))
}

fn keys(world: &mut World) {
    let typing = world
        .get_resource::<engine_ui::UiInputCapture>()
        .is_some_and(|c| c.keyboard);
    if typing {
        return;
    }
    let k = world.resource::<ButtonInput<KeyCode>>().clone();
    let (sh, ct) = (shift(world), ctrl(world));
    if let Some(g) = digit(&k) {
        if ct {
            let sel = world.resource::<Helm>().selected.clone();
            if !sel.is_empty() {
                world.resource_mut::<Helm>().groups[g] = sel.clone();
                tell(
                    world,
                    &format!("Group {g}"),
                    &format!(
                        "{} craft. Press {g} to pick them, twice to look at them.",
                        sel.len()
                    ),
                );
            }
        } else if sh {
            let sel = world.resource::<Helm>().selected.clone();
            let mut h = world.resource_mut::<Helm>();
            for e in sel {
                if !h.groups[g].contains(&e) {
                    h.groups[g].push(e);
                }
            }
        } else {
            recall(world, g);
        }
    }
    let any = !world.resource::<Helm>().selected.is_empty();
    if any && k.just_pressed(KeyCode::F) {
        order_all(world, "scout");
    }
    if any && k.just_pressed(KeyCode::B) {
        order_all(world, "home");
    }
    if any && k.just_pressed(KeyCode::Backspace) {
        order_all(world, "stop");
    }
    if any
        && k.just_pressed(KeyCode::Escape)
        && *world.resource::<crate::tools::Tool>() == crate::tools::Tool::Select
    {
        clear(world);
    }
    if k.just_pressed(KeyCode::Period) {
        next_idle(world);
    }
}

/// Pick the next of the player's craft lying idle, and look at her.
fn next_idle(world: &mut World) {
    let mut idle: Vec<(Entity, u32)> = {
        let mut q = world.query::<(Entity, &Vessel)>();
        q.iter(world)
            .filter(|(_, v)| v.commanded() && v.route.is_none())
            .filter(|(_, v)| matches!(v.roam, Roam::Idle { .. }) && v.doing != Doing::Sailing)
            .map(|(e, _)| (e, e.index_u32()))
            .collect()
    };
    if idle.is_empty() {
        tell(world, "No idle craft", "Every craft of yours has work.");
        return;
    }
    idle.sort_by_key(|x| x.1);
    let cur = world
        .resource::<Helm>()
        .selected
        .first()
        .map(|e| e.index_u32());
    let next = idle
        .iter()
        .find(|x| cur.is_none_or(|c| x.1 > c))
        .unwrap_or(&idle[0])
        .0;
    pick(world, vec![next], false);
    look_at(world, &[next]);
}

/// What a craft is doing, in words.
fn status(world: &World, v: &Vessel) -> (String, [f32; 3]) {
    let here = DVec2::new(v.at.x, v.at.z);
    if let Some(r) = v.route {
        let name = world
            .get_resource::<Routes>()
            .and_then(|rs| rs.0.get(r).map(|r| r.name.clone()))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "a trade".into());
        return (format!("On the {name} route"), CREAM);
    }
    match &v.roam {
        Roam::Scout { .. } => ("Scouting the fog".into(), AMBER),
        Roam::Found { isle, .. } => (
            format!("Bound to settle {}", island::islands()[*isle].name),
            CREAM,
        ),
        Roam::Circling { points } => {
            let c = points.iter().fold(DVec3::ZERO, |a, p| a + *p) / points.len().max(1) as f64;
            let w = waters(DVec2::new(c.x, c.z));
            (format!("Circling over {w}"), CREAM)
        }
        Roam::Idle { at } => {
            let to = DVec2::new(at.x, at.z);
            let d = (to - here).length();
            if d > 120.0 && v.doing == Doing::Sailing {
                (
                    format!("Sailing for {} · {:.1} km", waters(to), d / 1000.0),
                    GREEN,
                )
            } else if v.craft.flies() {
                (format!("Holding over {}", waters(here)), CREAM)
            } else {
                (format!("Lying in {}", waters(here)), CREAM)
            }
        }
        _ => (String::new(), CREAM),
    }
}

/// The craft's own picture (the yard card's set).
fn craft_icon(c: Craft) -> &'static str {
    match c {
        Craft::Schooner => "isle_craft_schooner",
        Craft::Steamer => "isle_craft_steamer",
        Craft::Smack => "isle_craft_smack",
        Craft::Barque => "isle_craft_barque",
        Craft::Airship => "isle_airship",
        Craft::Biplane => "isle_craft_biplane",
        Craft::Clipper => "isle_craft_clipper",
        Craft::PaddleSteamer => "isle_craft_paddle_steamer",
        Craft::Seaplane => "isle_craft_seaplane",
        Craft::Cutter => "isle_craft_cutter",
        Craft::Brig => "isle_craft_brig",
        Craft::Frigate => "isle_craft_frigate",
        Craft::Gunboat => "isle_craft_gunboat",
        Craft::Liner => "isle_craft_liner",
        Craft::FlyingBoat => "isle_craft_flying_boat",
        Craft::Dreadnought => "isle_craft_dreadnought",
        Craft::Leviathan => "isle_craft_leviathan",
        Craft::SkyCarrier => "isle_craft_sky_carrier",
    }
}

/// The card: the picked craft, their groups, and the orders.
fn card(world: &mut World) -> Option<El> {
    let h = world.resource::<Helm>();
    let sel = h.selected.clone();
    if sel.is_empty() {
        return None;
    }
    let groups: Vec<(usize, usize, bool)> = h
        .groups
        .iter()
        .enumerate()
        .filter(|(_, g)| !g.is_empty())
        .map(|(i, g)| (i, g.len(), *g == sel))
        .collect();
    let rows: Vec<(Entity, Vessel)> = sel
        .iter()
        .filter_map(|e| world.get::<Vessel>(*e).map(|v| (*e, v.clone())))
        .collect();
    let flying = rows.iter().filter(|r| r.1.craft.flies()).count();
    let ships = rows.len() - flying;
    let title = match (ships, flying) {
        (1, 0) | (0, 1) => rows[0].1.name.clone(),
        (s, 0) => format!("{s} ships"),
        (0, a) => format!("{a} aircraft"),
        (s, a) => format!("{s} ships, {a} aircraft"),
    };
    let group_of = |e: Entity| -> Option<usize> {
        world
            .resource::<Helm>()
            .groups
            .iter()
            .position(|g| g.contains(&e))
    };
    let mut list = El::new().class("col").with_style(|s| s.gap(4.0));
    for (i, (e, v)) in rows.iter().take(LISTED).enumerate() {
        let (what, col) = status(world, v);
        let g = group_of(*e);
        list = list.child(
            El::new()
                .class("row")
                .on_click(format!("isle.helm.only:{i}"))
                .tooltip(v.name.clone(), "Click to pick only her and look at her.")
                .with_style(|s| {
                    s.gap(10.0)
                        .align_items(Align::Center)
                        .padding([3.0, 6.0])
                        .radius(6.0)
                })
                .child(brass::badge(craft_icon(v.craft), 34.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).width(250.0).shrink(0.0))
                        .child(
                            El::text(v.name.clone())
                                .class("ink")
                                .text_style(|t| t.size(16.0).bold().nowrap())
                                .pick_ignore(),
                        )
                        .child(
                            El::text(what)
                                .class("ink")
                                .text_style(|t| t.size(13.0).italic().nowrap())
                                .with_style(move |s| {
                                    s.color(if col == CREAM {
                                        Colors::ink_dim()
                                    } else if col == AMBER {
                                        engine_ui::Color::hex(0x9a5a00)
                                    } else {
                                        Colors::good_ink()
                                    })
                                })
                                .pick_ignore(),
                        ),
                )
                .child_opt(g.map(|g| {
                    El::text(format!("{g}"))
                        .text_style(|t| t.size(15.0).bold().nowrap())
                        .with_style(|s| {
                            s.color(Colors::cream())
                                .background(Fill::Solid(Colors::brass_dark()))
                                .radius(11.0)
                                .padding([1.0, 8.0])
                        })
                        .tooltip("Control group", format!("Press {g} to pick this group."))
                })),
        );
    }
    if rows.len() > LISTED {
        list = list.child(
            El::text(format!("and {} more", rows.len() - LISTED))
                .class("ink")
                .text_style(|t| t.size(13.0).italic())
                .with_style(|s| s.color(Colors::ink_dim()).padding([0.0, 50.0]))
                .pick_ignore(),
        );
    }
    let button = |label: &str, key: &str, action: &str, tip: &str| {
        El::button(format!("{label}  ·  {key}"), format!("isle.helm.{action}"))
            .class("brass_button")
            .tooltip(label.to_string(), format!("{tip}\n[b]Key[/b]  {key}"))
    };
    let chips = El::new()
        .class("row")
        .with_style(|s| s.gap(6.0).align_items(Align::Center).wrap(true))
        .child(brass::label("Groups", true))
        .children(groups.iter().map(|(g, n, this)| {
            let this = *this;
            El::new()
                .on_click(format!("isle.helm.group:{g}"))
                .tooltip(
                    format!("Group {g}"),
                    format!("{n} craft. Press {g} to pick them, twice to look at them."),
                )
                .with_style(move |s| {
                    s.padding([2.0, 10.0])
                        .radius(12.0)
                        .background(Fill::Solid(if this {
                            Colors::brass()
                        } else {
                            Colors::brass().with_alpha(0.25)
                        }))
                })
                .child(
                    El::text(format!("{g} · {n}"))
                        .text_style(|t| t.size(14.0).bold().nowrap())
                        .with_style(move |s| {
                            s.color(if this {
                                Colors::ink()
                            } else {
                                Colors::ink_dim()
                            })
                        })
                        .pick_ignore(),
                )
        }))
        .child_opt(groups.is_empty().then(|| {
            El::text("Ctrl and a number makes one")
                .class("ink")
                .text_style(|t| t.size(13.0).italic().nowrap())
                .with_style(|s| s.color(Colors::ink_dim()))
                .pick_ignore()
        }));
    Some(
        brass::parchment()
            .with_style(|s| {
                s.width(470.0)
                    .padding([14.0, 18.0])
                    .gap(10.0)
                    .direction(Direction::Column)
            })
            .child(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(10.0).align_items(Align::Center))
                    .child(brass::badge("isle_compass", 40.0))
                    .child(
                        El::new()
                            .class("col")
                            .with_style(|s| s.gap(0.0).grow(1.0))
                            .child(
                                El::text(title)
                                    .class("ink_title")
                                    .text_style(|t| t.nowrap())
                                    .pick_ignore(),
                            )
                            .child(
                                El::text("Right click the sea to send them")
                                    .class("ink")
                                    .text_style(|t| t.size(13.0).italic().nowrap())
                                    .with_style(|s| s.color(Colors::ink_dim()))
                                    .pick_ignore(),
                            ),
                    )
                    .child(
                        brass::medallion("isle_close", 30.0, "isle.helm.clear")
                            .tooltip("Put down", "Pick nothing.\n[b]Key[/b]  Esc"),
                    ),
            )
            .child(brass::divider(430.0))
            .child(list)
            .child(chips)
            .child(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(8.0).wrap(true))
                    .child(button(
                        "Scout",
                        "F",
                        "scout",
                        "Chart the fog: each sails for the nearest sea nobody has seen, then the next, and puts back to harbour when none is left in reach.",
                    ))
                    .child(button(
                        "Stop",
                        "Backspace",
                        "stop",
                        "Heave to where they are and wait for orders.",
                    ))
                    .child(button(
                        "Home",
                        "B",
                        "home",
                        "Put back to the nearest harbour of yours (aircraft to a mast).",
                    )),
            ),
    )
}

fn refresh_card(world: &mut World) {
    let sig = {
        let h = world.resource::<Helm>();
        let mut s = String::new();
        for e in &h.selected {
            s.push_str(&format!("{};", e.index_u32()));
        }
        for g in &h.groups {
            s.push_str(&format!("|{}", g.len()));
        }
        s
    };
    let statuses: String = {
        let sel = world.resource::<Helm>().selected.clone();
        sel.iter()
            .take(LISTED)
            .filter_map(|e| world.get::<Vessel>(*e).map(|v| status(world, v).0))
            .collect::<Vec<_>>()
            .join(",")
    };
    let sig = format!("{sig}#{statuses}");
    if world.resource::<Helm>().sig == sig {
        return;
    }
    let Some(node) = engine_ui::input::find_id(world, CARD_ID) else {
        return;
    };
    let els: Vec<El> = card(world).into_iter().collect();
    let _ = engine_ui::el::replace_children(world, node, &els);
    world.resource_mut::<Helm>().sig = sig;
}

/// The selection box on the screen while the left button drags.
fn draw_box(world: &mut World) {
    let (press, boxing) = {
        let h = world.resource::<Helm>();
        (h.press, h.boxing)
    };
    let px = world.resource::<Mouse>().position;
    let scale = world
        .get_resource::<engine_ui::layout::UiViewport>()
        .map_or(1.0, |v| v.scale.max(0.01));
    let Some(node) = engine_ui::input::find_id(world, BOX_ID) else {
        return;
    };
    let Some(mut n) = world.get_mut::<engine_ui::node::Node>(node) else {
        return;
    };
    match (press, px, boxing) {
        (Some(a), Some(b), true) => {
            let (lo, hi) = (a.min(b) / scale, a.max(b) / scale);
            n.style.left = Some(Val::Px(lo.x));
            n.style.top = Some(Val::Px(lo.y));
            n.style.width = Some(Val::Px(hi.x - lo.x));
            n.style.height = Some(Val::Px(hi.y - lo.y));
            n.style.opacity = Some(1.0);
        }
        _ => {
            if n.style.opacity != Some(0.0) {
                n.style.opacity = Some(0.0);
            }
        }
    }
}

// ------------------------------------------------------------------------------ marks on the sea

/// Flat marks on the sea, built in the planet's frame.
struct Marks<'a> {
    frame: &'a Frame,
    anchor: DVec3,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

impl Marks<'_> {
    fn quad(&mut self, p: [DVec3; 4], color: [f32; 4]) {
        let base = self.vertices.len() as u32;
        let up = (self.frame.at(p[0] + DVec3::Y) - self.frame.at(p[0]))
            .normalize()
            .as_vec3();
        for q in p {
            let w = self.frame.at(q) - self.anchor;
            self.vertices.push(Vertex {
                position: w.as_vec3().to_array(),
                normal: up.to_array(),
                uv: [0.0, 0.0],
                color,
            });
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A ring round map point `c` (its `y` the height), `dashes` > 0 for a dashed one.
    fn ring(&mut self, c: DVec3, r: f64, width: f64, color: [f32; 4], dashes: usize) {
        let n = if dashes > 0 { dashes * 2 } else { 48 };
        for i in 0..n {
            if dashes > 0 && i % 2 == 1 {
                continue;
            }
            let a0 = i as f64 / n as f64 * std::f64::consts::TAU;
            let a1 = (i + 1) as f64 / n as f64 * std::f64::consts::TAU;
            let at = |a: f64, rr: f64| DVec3::new(c.x + a.cos() * rr, c.y, c.z + a.sin() * rr);
            let (ri, ro) = (r - width * 0.5, r + width * 0.5);
            self.quad([at(a0, ri), at(a0, ro), at(a1, ro), at(a1, ri)], color);
        }
    }

    /// A dashed line along `pts` (map points), from `skip` meters in.
    fn dashes(&mut self, pts: &[DVec3], skip: f64, dash: f64, width: f64, y: f64, color: [f32; 4]) {
        let mut along = 0.0f64;
        let mut drawn = 0;
        let period = dash * 1.8;
        for w in pts.windows(2) {
            let (a, b) = (DVec2::new(w[0].x, w[0].z), DVec2::new(w[1].x, w[1].z));
            let len = (b - a).length();
            if len < 1e-6 {
                continue;
            }
            let d = (b - a) / len;
            let side = DVec2::new(-d.y, d.x) * width * 0.5;
            // Dashes start at whole periods from the line's start, so they don't crawl.
            let mut t = ((along.max(skip) / period).ceil() * period - along).max(0.0);
            while t < len && drawn < 600 {
                let e = (t + dash).min(len);
                let p0 = a + d * t;
                let p1 = a + d * e;
                let m = |p: DVec2| DVec3::new(p.x, y, p.y);
                self.quad(
                    [m(p0 - side), m(p0 + side), m(p1 + side), m(p1 - side)],
                    color,
                );
                drawn += 1;
                t += period;
            }
            along += len;
        }
    }
}

fn overlay_material(world: &mut World) -> Handle<StandardMaterial> {
    world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: [1.0, 1.0, 1.0, 1.0],
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            ..Default::default()
        })
}

/// Rings under the picked craft and the one under the cursor, their courses, and the flashes
/// of new orders.
fn draw_marks(world: &mut World, dt: f32) {
    let frame = *world.resource::<Frame>();
    let Some(v) = view(world) else {
        return;
    };
    let hover = world.resource::<Helm>().hover;
    let selected = world.resource::<Helm>().selected.clone();
    let anchor = v.t.translation;
    let mut marks = Marks {
        frame: &frame,
        anchor,
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    // Sizes keep to a few pixels at every zoom.
    let zoom = v.dist as f64;
    let ring_of = |c: Craft| (c.half_length() as f64 * 1.5).max(zoom * 0.016);
    for e in selected.iter().chain(hover.iter()) {
        let picked = selected.contains(e);
        if !picked && Some(*e) != hover {
            continue;
        }
        let Some(x) = world.get::<Vessel>(*e) else {
            continue;
        };
        let y = if x.craft.flies() { x.at.y - 3.0 } else { 1.4 };
        let r = ring_of(x.craft);
        if !picked {
            marks.ring(x.at.with_y(y), r, r * 0.12, [1.0, 1.0, 1.0, 0.55], 0);
            continue;
        }
        let scouting = matches!(x.roam, Roam::Scout { .. });
        let col = if scouting { AMBER } else { GREEN };
        marks.ring(
            x.at.with_y(y),
            r,
            r * 0.18,
            [col[0], col[1], col[2], 0.95],
            0,
        );
        // A dreadnought's convoy ring.
        if x.craft == Craft::Dreadnought {
            let w = (zoom * 0.003).max(4.0);
            marks.ring(
                x.at.with_y(1.3),
                crate::flagships::ESCORT_M as f64,
                w,
                [GOLD[0], GOLD[1], GOLD[2], 0.7],
                40,
            );
        }
        // Her course ahead, and where it ends.
        if x.doing == Doing::Sailing && x.path.len() >= 2 {
            let course_col = if x.route.is_some() { CREAM } else { col };
            let dash = (zoom * 0.012).max(10.0);
            let width = (zoom * 0.0026).max(2.0);
            let ly = if x.craft.flies() { y } else { 1.2 };
            let pts: Vec<DVec3> = x.path.iter().copied().collect();
            marks.dashes(
                &pts,
                x.s + r,
                dash,
                width,
                ly,
                [course_col[0], course_col[1], course_col[2], 0.8],
            );
            if let Some(end) = x.path.last() {
                let ey = if x.craft.flies() { end.y } else { 1.2 };
                marks.ring(
                    end.with_y(ey),
                    r * 0.7,
                    r * 0.12,
                    [course_col[0], course_col[1], course_col[2], 0.85],
                    10,
                );
            }
        }
    }
    // Order flashes: a ring that opens out and fades.
    let mut pings = std::mem::take(&mut world.resource_mut::<Helm>().pings);
    for p in &mut pings {
        p.age += dt;
        let k = (p.age / 0.9).clamp(0.0, 1.0) as f64;
        let r = zoom * 0.02 * (0.4 + 1.4 * k);
        let a = (1.0 - k) as f32;
        marks.ring(
            p.at.with_y(1.6),
            r,
            r * 0.16,
            [p.color[0], p.color[1], p.color[2], a],
            0,
        );
    }
    pings.retain(|p| p.age < 0.9);
    world.resource_mut::<Helm>().pings = pings;
    let (vertices, indices) = (marks.vertices, marks.indices);
    let material = match world.resource::<Helm>().material {
        Some(m) => m,
        None => {
            let m = overlay_material(world);
            world.resource_mut::<Helm>().material = Some(m);
            m
        }
    };
    let overlay = world.resource::<Helm>().overlay;
    match (overlay, indices.is_empty()) {
        (Some((e, _)), true) => {
            if let Some(mut r) = world.get_mut::<MeshRenderer>(e)
                && r.visible
            {
                r.visible = false;
            }
        }
        (Some((e, handle)), false) => {
            world
                .resource_mut::<Assets<Mesh>>()
                .set(handle, Mesh::new(vertices, indices));
            if let Some(mut r) = world.get_mut::<MeshRenderer>(e) {
                r.visible = true;
            }
            if let Some(mut t) = world.get_mut::<Transform>(e) {
                *t = Transform::from_translation(anchor);
            }
        }
        (None, false) => {
            let handle = world
                .resource_mut::<Assets<Mesh>>()
                .add(Mesh::new(vertices, indices));
            let mut r = MeshRenderer::new(handle, material);
            r.cast_shadows = false;
            let e = world
                .spawn((
                    Name::new("Helm marks"),
                    Transform::from_translation(anchor),
                    r,
                    NoVirtualGeometry,
                    NoMeshLod,
                ))
                .id();
            world.resource_mut::<Helm>().overlay = Some((e, handle));
        }
        (None, true) => {}
    }
}

/// Each frame: drop craft that are gone or no longer the player's, keys, the craft under the
/// cursor, the box, the card and the marks.
pub fn update(world: &mut World) {
    if !world.contains_resource::<Helm>() {
        return;
    }
    let dt = world.resource::<Time>().real_delta_secs();
    world.resource_mut::<Helm>().clock += dt;
    // Forget what is gone.
    {
        let alive =
            |world: &World, e: &Entity| world.get::<Vessel>(*e).is_some_and(|v| v.commanded());
        let h = world.resource::<Helm>();
        let sel: Vec<Entity> = h
            .selected
            .iter()
            .copied()
            .filter(|e| alive(world, e))
            .collect();
        let groups: Vec<Vec<Entity>> = h
            .groups
            .iter()
            .map(|g| g.iter().copied().filter(|e| alive(world, e)).collect())
            .collect();
        let mut h = world.resource_mut::<Helm>();
        h.selected = sel;
        for (i, g) in groups.into_iter().enumerate() {
            h.groups[i] = g;
        }
    }
    // A building picked from the map puts the craft down.
    if world
        .get_resource::<crate::tools::Hand>()
        .is_some_and(|h| h.selected.is_some())
    {
        clear(world);
    }
    keys(world);
    let over_ui = world
        .get_resource::<engine_ui::UiInputCapture>()
        .is_some_and(|c| c.pointer);
    let select_tool = world
        .get_resource::<crate::tools::Tool>()
        .is_some_and(|t| *t == crate::tools::Tool::Select);
    let hover = match world.resource::<Mouse>().position {
        Some(px) if !over_ui && select_tool && !world.resource::<EngineConfig>().headless => {
            ship_under(world, px)
        }
        _ => None,
    };
    world.resource_mut::<Helm>().hover = hover;
    draw_box(world);
    refresh_card(world);
    draw_marks(world, dt);
}

fn tell(world: &mut World, title: &str, text: &str) {
    if world.get_resource::<engine_ui::theme::Theme>().is_none() {
        return;
    }
    engine_ui::toast::notify(
        world,
        engine_ui::toast::ToastKind::Info,
        "isle_compass",
        title,
        text,
        5.0,
    );
}

/// `isle.helm.*` for scripts and the playtest: pick craft, give orders, see the groups.
pub fn register(app: &mut App) {
    use serde_json::{Value, json};
    fn entities(world: &mut World, args: &Value) -> Result<Vec<Entity>, String> {
        let list = args["vessels"]
            .as_array()
            .ok_or("vessels wants a list of names or ids")?;
        list.iter()
            .map(|x| {
                let key = match x {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => return Err("a vessel is a name or an id".to_string()),
                };
                crate::orders::find_vessel(world, &key).ok_or(format!("no vessel {key}"))
            })
            .collect()
    }
    app.register_api(
        "isle.helm.pick",
        "Pick craft of yours, as a box or a click would (add: keep the ones picked already).",
        r#"{"vessels": ["Fair Venture"], "add": false}"#,
        |world, args| {
            let list = entities(world, &args)?;
            let add = args["add"].as_bool().unwrap_or(false);
            pick(world, list, add);
            Ok(json!({"picked": world.resource::<Helm>().selected.len()}))
        },
    )
    .register_api(
        "isle.helm.group",
        "Make the picked craft control group n (0..9), as Ctrl and a number does.",
        r#"{"group": 1}"#,
        |world, args| {
            let g = args["group"].as_u64().ok_or("group wants 0..9")? as usize;
            let sel = world.resource::<Helm>().selected.clone();
            world.resource_mut::<Helm>().groups[g.min(9)] = sel;
            Ok(json!({"ok": true}))
        },
    )
    .register_api(
        "isle.helm.send",
        "Send the picked craft to a map point in formation, as a right click on the sea does.",
        r#"{"to": [1200.0, -800.0]}"#,
        |world, args| {
            let to = DVec2::new(
                args["to"][0].as_f64().ok_or("to wants [x, z]")?,
                args["to"][1].as_f64().ok_or("to wants [x, z]")?,
            );
            let list = world.resource::<Helm>().selected.clone();
            send(world, &list, to);
            Ok(json!({"ok": true}))
        },
    )
    .register_api(
        "isle.helm.order",
        "Give the picked craft an order: scout, stop or home.",
        r#"{"order": "scout"}"#,
        |world, args| {
            let o = args["order"]
                .as_str()
                .ok_or("order wants scout, stop or home")?;
            if !matches!(o, "scout" | "stop" | "home") {
                return Err(format!("unknown order {o}"));
            }
            order_all(world, o);
            Ok(json!({"ok": true}))
        },
    );
}

// ------------------------------------------------------------------------------ pictures

/// Run frames until the ground and the trees are in (or `cap_s` of wall time passes).
fn settle_view(app: &mut App, cap_s: f32) {
    let started = std::time::Instant::now();
    loop {
        app.update();
        let w = &app.world;
        let (building, missing) = w
            .get_resource::<engine::core::diagnostics::Diagnostics>()
            .map_or((0.0, 0.0), |d| {
                (
                    d.get("terrain.patches_building").unwrap_or(0.0),
                    d.get("terrain.patches_missing").unwrap_or(0.0),
                )
            });
        let foliage = w
            .get_resource::<engine_foliage::FoliageLoading>()
            .is_some_and(|f| !f.is_ready());
        let elapsed = started.elapsed().as_secs_f32();
        if (building + missing == 0.0 && !foliage && elapsed > 0.5) || elapsed > cap_s {
            break;
        }
    }
}

/// Game seconds of play, not drawn.
fn play(app: &mut App, game_s: f32, speed: f32) {
    use engine::render::RenderPaused;
    app.world.insert_resource(RenderPaused);
    app.world.resource_mut::<crate::sim::Economy>().speed = speed;
    let frames = (game_s / speed * 30.0).ceil() as usize;
    app.update_n(frames.max(1));
    app.world.resource_mut::<crate::sim::Economy>().speed = 1.0;
    app.world.remove_resource::<RenderPaused>();
}

fn aim(world: &mut World, at: DVec2, yaw: f32, dist: f32, pitch_deg: Option<f32>) {
    let y = island::height(at.x, at.y).max(0.0);
    let mut q = world.query::<&mut StrategyCam>();
    if let Some(mut cam) = q.iter_mut(world).next() {
        cam.goal_focus = DVec3::new(at.x, y, at.y);
        cam.goal_yaw = yaw;
        cam.goal_dist = dist;
        cam.pitch_override = pitch_deg.map(f32::to_radians);
        cam.free_height = false;
        cam.snap();
    }
}

fn shoot(app: &mut App, out: &str, key: &str, only: &Option<Vec<String>>) {
    if only.as_ref().is_some_and(|k| !k.iter().any(|x| x == key)) {
        return;
    }
    let cap = std::env::var("ARCH_TOUR_SETTLE_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(240.0);
    settle_view(app, cap);
    for _ in 0..6 {
        app.update();
    }
    if let Some(d) = app.world.get_resource::<engine::render::RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    let path = format!("{out}/{key}.png");
    match engine::render::capture_now(&mut app.world, &path) {
        Ok(_) => engine::core::log::info!("helm shots: {path}"),
        Err(e) => engine::core::log::warn!("helm shots: {path}: {e}"),
    }
}

/// A harbour site on island `i` near its landfall.
fn harbour_site(world: &World, i: usize) -> Option<(IVec2, u8)> {
    let p = island::settle_coast(&island::islands()[i])?;
    let t = crate::grid::cell_of(p.x, p.y);
    let g = world.resource::<crate::grid::Grid>();
    for r in 0i32..60 {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dz.abs() != r {
                    continue;
                }
                for rot in 0..4u8 {
                    let o = t + IVec2::new(dx, dz);
                    if crate::grid::check(g, Kind::Harbour, o, rot) == Ok(i) {
                        return Some((o, rot));
                    }
                }
            }
        }
    }
    None
}

/// For the pictures: a new game, headless on a fixed clock, Haverholm settled with the barque
/// of the start. Returns the new harbour's mooring.
fn new_game_settled(app: &mut App, out: &str) -> Option<DVec2> {
    let _ = std::fs::create_dir_all(out);
    app.world.resource_mut::<EngineConfig>().headless = true;
    if let Some(mut i) = app.world.get_resource_mut::<crate::incidents::Incidents>() {
        i.rolls = false;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(engine::core::time::TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    play(app, 2.0, 1.0);
    let world = &mut app.world;
    let Some((o, rot)) = harbour_site(world, 0) else {
        engine::core::log::warn!("shots: no harbour site on Haverholm");
        return None;
    };
    if let Err(e) = crate::build::place(world, Kind::Harbour, o, rot, true) {
        engine::core::log::warn!("shots: {e}");
    }
    for _ in 0..40 {
        if !app.world.resource::<crate::settle::Settled>().0.is_empty() {
            break;
        }
        play(app, 30.0, 8.0);
    }
    let world = &mut app.world;
    let mut q = world.query::<&Building>();
    let quay = q
        .iter(world)
        .find(|b| b.kind == Kind::Harbour && b.island == 0)
        .map(|b| crate::fleet::mooring(b).0);
    if quay.is_none() {
        engine::core::log::warn!("shots: Haverholm was not settled");
    }
    quay
}

/// `--helm-shots [keys]`: a new game, Haverholm settled and a small fleet off it, then the
/// helm's pictures into `screenshots/archipelago/helm/`: a box being drawn round the fleet
/// (`helm_box`), the fleet picked and sent (`helm_send`), two scouts at work over the fog
/// (`helm_scout`), the home isles from high up after they have charted it (`helm_charted`).
pub fn shots(mut app: App, only: Option<&str>) {
    let out = "screenshots/archipelago/helm";
    let only: Option<Vec<String>> = only.map(|s| s.split(',').map(str::to_string).collect());
    let Some(quay) = new_game_settled(&mut app, out) else {
        return;
    };
    let world = &mut app.world;
    let out_dir = (quay - island::islands()[0].center).normalize_or_zero();
    let side = DVec2::new(-out_dir.y, out_dir.x);
    let fleet = [
        (Craft::Schooner, "Kittiwake"),
        (Craft::Brig, "Merrow"),
        (Craft::Cutter, "Swift"),
        (Craft::Clipper, "Albatross"),
        (Craft::Steamer, "Ironside"),
    ];
    for (k, (craft, name)) in fleet.into_iter().enumerate() {
        let p = quay + out_dir * (260.0 + 110.0 * (k / 3) as f64) + side * (k % 3) as f64 * 110.0
            - side * 110.0;
        let at = DVec3::new(p.x, 0.0, p.y);
        let heading = (out_dir.y).atan2(out_dir.x) as f32;
        let e = crate::fleet::spawn_vessel(
            world,
            craft,
            name,
            at,
            -heading,
            None,
            Roam::Idle { at },
            900 + k as u64,
        );
        if let Some(mut v) = world.get_mut::<Vessel>(e) {
            v.doing = Doing::Moored(60.0);
        }
    }
    play(&mut app, 4.0, 1.0);
    let names = ["Kittiwake", "Merrow", "Swift", "Albatross", "Ironside"];
    let ents: Vec<Entity> = names
        .iter()
        .filter_map(|n| crate::orders::find_vessel(&mut app.world, n))
        .collect();
    let mid = {
        let pts: Vec<DVec3> = ents
            .iter()
            .filter_map(|e| app.world.get::<Vessel>(*e).map(|v| v.at))
            .collect();
        pts.iter().fold(DVec3::ZERO, |a, p| a + *p) / pts.len().max(1) as f64
    };
    let mid2 = DVec2::new(mid.x, mid.z);
    // 1. A box being drawn round them.
    aim(&mut app.world, mid2, 0.6, 520.0, Some(48.0));
    settle_view(&mut app, 30.0);
    if let Some(v) = view(&mut app.world) {
        let shots: Vec<Vec2> = on_screen(&mut app.world, &v)
            .into_iter()
            .filter(|(e, _, _)| ents.contains(e))
            .map(|x| x.1)
            .collect();
        if !shots.is_empty() {
            let lo = shots.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p)) - 70.0;
            let hi = shots.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p)) + 50.0;
            app.world.resource_mut::<Mouse>().position = Some(hi);
            let mut h = app.world.resource_mut::<Helm>();
            h.press = Some(lo);
            h.boxing = true;
        }
    }
    shoot(&mut app, out, "helm_box", &only);
    {
        let mut h = app.world.resource_mut::<Helm>();
        h.press = None;
        h.boxing = false;
    }
    // 2. Picked, made group 1, and sent across the strait.
    pick(&mut app.world, ents.clone(), false);
    app.world.resource_mut::<Helm>().groups[1] = ents.clone();
    let to = mid2 + out_dir * 1_400.0 + side * 600.0;
    send(&mut app.world, &ents, to);
    play(&mut app, 20.0, 1.0);
    aim(
        &mut app.world,
        mid2 + out_dir * 500.0 + side * 250.0,
        0.6,
        1_500.0,
        Some(52.0),
    );
    shoot(&mut app, out, "helm_send", &only);
    // 3. Two of them scouting the fog.
    let scouts = vec![ents[2], ents[3]];
    pick(&mut app.world, scouts.clone(), false);
    order_all(&mut app.world, "scout");
    play(&mut app, 240.0, 8.0);
    let s_mid = scouts
        .iter()
        .filter_map(|e| {
            app.world
                .get::<Vessel>(*e)
                .map(|v| DVec2::new(v.at.x, v.at.z))
        })
        .fold(DVec2::ZERO, |a, p| a + p)
        / 2.0;
    aim(&mut app.world, s_mid, 0.35, 5_500.0, Some(50.0));
    shoot(&mut app, out, "helm_scout", &only);
    // 4. The home isles from high up once they have been at it a while.
    play(&mut app, 600.0, 8.0);
    clear(&mut app.world);
    aim(
        &mut app.world,
        DVec2::new(0.0, 0.0),
        0.35,
        9_000.0,
        Some(50.0),
    );
    shoot(&mut app, out, "helm_charted", &only);
    let share = app
        .world
        .get_resource::<FogOfWar>()
        .map_or(0.0, |f| f.share_seen());
    engine::core::log::info!(
        "helm shots: done; {:.2} % of the planet seen",
        share * 100.0
    );
}

/// `--flagship-shots [keys]`: a new game with Haverholm settled, then the flagships at work,
/// into `screenshots/archipelago/flagships/`: a dreadnought leading a convoy across the
/// strait with her ring shown (`flag_convoy`), her close by under way (`flag_dreadnought`),
/// a leviathan off the new quay (`flag_leviathan`), a sky carrier with her escorts over the
/// island (`flag_carrier`).
pub fn flagship_shots(mut app: App, only: Option<&str>) {
    let out = "screenshots/archipelago/flagships";
    let only: Option<Vec<String>> = only.map(|s| s.split(',').map(str::to_string).collect());
    let Some(quay) = new_game_settled(&mut app, out) else {
        return;
    };
    // The ships, not the build bar, fill the lower half of these.
    let shoot = |app: &mut App, out: &str, key: &str, only: &Option<Vec<String>>| {
        let mut q = app
            .world
            .query::<(&engine_ui::node::Screen, &mut engine_ui::node::Node)>();
        for (s, mut node) in q.iter_mut(&mut app.world) {
            if s.key == "isle_bar" {
                node.style.opacity = Some(0.0);
            }
        }
        shoot(app, out, key, only);
    };
    let out_dir = (quay - island::islands()[0].center).normalize_or_zero();
    let side = DVec2::new(-out_dir.y, out_dir.x);
    let heading = -(out_dir.y.atan2(out_dir.x) as f32);
    let spawn = |world: &mut World, craft: Craft, name: &str, p: DVec2, y: f64, h: f32, k: u64| {
        let at = DVec3::new(p.x, y, p.y);
        let e = crate::fleet::spawn_vessel(world, craft, name, at, h, None, Roam::Idle { at }, k);
        if let Some(mut v) = world.get_mut::<Vessel>(e) {
            v.doing = Doing::Moored(60.0);
        }
        e
    };
    let world = &mut app.world;
    // The convoy: the dreadnought and four traders in two columns behind her.
    let lead = quay + out_dir * 420.0;
    let dread = spawn(
        world,
        Craft::Dreadnought,
        "Stormcrown",
        lead,
        0.0,
        heading,
        950,
    );
    let mut convoy = vec![dread];
    for (k, craft) in [Craft::Steamer, Craft::Clipper, Craft::Barque, Craft::Brig]
        .into_iter()
        .enumerate()
    {
        let p = lead - side * 200.0 * (1 + k / 2) as f64
            + out_dir * if k % 2 == 0 { 110.0 } else { -110.0 };
        convoy.push(spawn(
            world,
            craft,
            craft.name(),
            p,
            0.0,
            heading,
            951 + k as u64,
        ));
    }
    // The leviathan off the quay, the carrier over the island behind it.
    let lev_at = quay + out_dir * 260.0 + side * 520.0;
    let lev = spawn(
        world,
        Craft::Leviathan,
        "Great Meridian",
        lev_at,
        0.0,
        heading + 1.4,
        960,
    );
    let sky_at = quay - out_dir * 350.0 - side * 300.0;
    let sky = spawn(
        world,
        Craft::SkyCarrier,
        "Cloud Citadel",
        sky_at,
        240.0,
        heading + 0.6,
        961,
    );
    play(&mut app, 6.0, 1.0);
    // 1. The convoy under way, her ring round them.
    pick(&mut app.world, convoy.clone(), false);
    let to = lead - side * 2_600.0 + out_dir * 900.0;
    send(&mut app.world, &convoy, to);
    play(&mut app, 50.0, 1.0);
    let at = |app: &mut App, e: Entity| {
        app.world
            .get::<Vessel>(e)
            .map_or(quay, |v| DVec2::new(v.at.x, v.at.z))
    };
    let d = at(&mut app, dread);
    aim(&mut app.world, d, 0.5, 3_600.0, Some(66.0));
    shoot(&mut app, out, "flag_convoy", &only);
    // 2. Her close by, smoking, the HUD and her card up.
    pick(&mut app.world, vec![dread], false);
    play(&mut app, 4.0, 1.0);
    let d = at(&mut app, dread);
    let h = app.world.get::<Vessel>(dread).map_or(0.0, |v| v.heading);
    aim(&mut app.world, d, -h + 0.75, 330.0, Some(15.0));
    shoot(&mut app, out, "flag_dreadnought", &only);
    // 3. The leviathan lying off the quay.
    clear(&mut app.world);
    let l = at(&mut app, lev);
    let h = app.world.get::<Vessel>(lev).map_or(0.0, |v| v.heading);
    aim(&mut app.world, l, -h + 0.6, 380.0, Some(20.0));
    shoot(&mut app, out, "flag_leviathan", &only);
    // 4. The carrier and her two fighters.
    pick(&mut app.world, vec![sky], false);
    play(&mut app, 3.0, 1.0);
    let (c, h) = app
        .world
        .get::<Vessel>(sky)
        .map_or((DVec3::ZERO, 0.0), |v| (v.at, v.heading));
    // Level with her, the island and the sea below.
    {
        let world = &mut app.world;
        let mut q = world.query::<&mut StrategyCam>();
        if let Some(mut cam) = q.iter_mut(world).next() {
            cam.goal_focus = c - DVec3::Y * 20.0;
            cam.goal_yaw = -h + 0.9;
            cam.goal_dist = 560.0;
            cam.pitch_override = Some(14f32.to_radians());
            cam.free_height = true;
            cam.snap();
        }
    }
    shoot(&mut app, out, "flag_carrier", &only);
    engine::core::log::info!("flagship shots: done");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formations_keep_their_distance() {
        let pts: Vec<DVec2> = (0..40).map(formation).collect();
        for (i, a) in pts.iter().enumerate() {
            for b in &pts[i + 1..] {
                assert!((*a - *b).length() > SPACING_M * 0.5, "{a} {b}");
            }
        }
        assert_eq!(pts[0], DVec2::ZERO);
        assert!(pts[6].length() < SPACING_M * 1.01);
        assert!(pts[7].length() > SPACING_M * 1.5);
    }

    #[test]
    fn a_scout_makes_for_the_nearest_fog_ahead_and_none_when_all_is_seen() {
        let fog = FogOfWar::default();
        // Open sea between the home isles, heading east (+x).
        let here = DVec2::new(0.0, -4_000.0);
        let c = candidates(&fog, here, 0.0, 1_800.0, false, &[]);
        let first = *c.first().expect("fog all round");
        assert!((first - here).length() < 1_800.0 * 1.7, "{first}");
        assert!(first.x > here.x, "ahead of her: {first}");
        // Another scout bound there already: this one goes elsewhere.
        let c2 = candidates(&fog, here, 0.0, 1_800.0, false, &[first]);
        assert!((c2[0] - first).length() > 1_800.0 * 1.5);
        // All seen: nowhere to go.
        let mut fog = FogOfWar::default();
        fog.reveal_map(here.x, here.y, 200_000.0);
        assert!(candidates(&fog, here, 0.0, 1_800.0, false, &[]).is_empty());
    }
}
