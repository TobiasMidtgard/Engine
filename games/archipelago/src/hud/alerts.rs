//! Alerts down the left edge: every fire, riot and sickness burning now, worst first, each a
//! brass strip with its picture, what and where, how long ago, and "Go there" (the view
//! glides to it and its card opens). New ones breathe; a cross puts one away until it ends.
//!
//! Data is the game's (`incidents::alerts`); this is the UI thread's.

use std::collections::HashSet;

use engine::prelude::*;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, AlertSpec, Colors, Urgency};
use engine_ui::node::Screen;
use engine_ui::style::{Direction, Pick, Position};

use crate::incidents::{Alert, Trouble};
use crate::sim::Economy;

pub const SCREEN: &str = "isle_alerts";
const LIST_ID: &str = "isle.alerts";
/// At most this many strips; the rest are counted under them.
const SHOWN: usize = 4;
/// Game seconds an alert counts as new (its ring breathes).
const FRESH_S: f64 = 30.0;

#[derive(Resource, Default)]
pub struct Alerts {
    /// Put away by the player (until the trouble ends).
    hidden: HashSet<u32>,
    /// On screen now (a rebuild doesn't slide these in again).
    seen: HashSet<u32>,
    sig: String,
    left: f32,
}

pub fn install(world: &mut World) {
    world.init_resource::<Alerts>();
    let screen = Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 1,
        closable: false,
        ..Default::default()
    };
    let el = El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(El::new().id(LIST_ID).with_style(|s| {
            s.position(Position::Absolute)
                .left(22.0)
                .top(150.0)
                .direction(Direction::Column)
                .gap(8.0)
                .pick(Pick::Ignore)
        }));
    if let Err(e) = engine_ui::screens::spawn_screen(world, screen, &el) {
        engine::core::log::warn!("archipelago alerts: {e}");
    }
}

fn picture(t: Trouble) -> &'static str {
    match t {
        Trouble::Fire => "isle_alert_fire",
        Trouble::Riot => "isle_alert_riot",
        Trouble::Sickness => "isle_alert_sick",
    }
}

fn title(a: &Alert) -> String {
    let kind = a.kind.def().name.to_lowercase();
    match a.what {
        Trouble::Fire => format!("Fire at the {kind}"),
        Trouble::Riot => format!("Riot at the {kind}"),
        Trouble::Sickness => format!("Sickness at the {kind}"),
    }
}

/// How bad: a fire always, the others once they have taken hold or nobody answers them.
fn urgency(a: &Alert) -> Urgency {
    if a.what == Trouble::Fire || a.level >= 0.6 || !a.crew {
        Urgency::Danger
    } else {
        Urgency::Warning
    }
}

fn age_id(id: u32) -> String {
    format!("isle.alert.age.{id}")
}

fn age(s: f64) -> String {
    if s < 20.0 {
        "just now".into()
    } else if s < 90.0 {
        format!("{:.0} s", s)
    } else {
        format!("{:.0} min", s / 60.0)
    }
}

/// Where the strips go: right of the item shelf and the gear screen when they are open.
fn left(world: &World) -> f32 {
    let mut x = 22.0;
    if world
        .get_resource::<super::shelf::Shelf>()
        .is_some_and(|s| s.open)
    {
        x = 478.0;
    }
    // The gear screen opens where the strips would be, 560 wide.
    if world
        .get_resource::<super::fleet::Fleet>()
        .is_some_and(|f| f.open)
    {
        x += 576.0;
    }
    x
}

fn shown(world: &mut World) -> (Vec<Alert>, usize) {
    let all = crate::incidents::alerts(world);
    // Forget put-away alerts once their trouble is over.
    let live: HashSet<u32> = all.iter().map(|a| a.id).collect();
    world
        .resource_mut::<Alerts>()
        .hidden
        .retain(|id| live.contains(id));
    let hidden = world.resource::<Alerts>().hidden.clone();
    let mut list: Vec<Alert> = all
        .into_iter()
        .filter(|a| !hidden.contains(&a.id))
        .collect();
    // Worst first, then the newest.
    // Worst first (fires before the rest), then the newest.
    let rank = |a: &Alert| (urgency(a) as u8, (a.what == Trouble::Fire) as u8);
    list.sort_by(|a, b| rank(b).cmp(&rank(a)).then(b.since_s.total_cmp(&a.since_s)));
    let more = list.len().saturating_sub(SHOWN);
    list.truncate(SHOWN);
    (list, more)
}

fn strips(list: &[Alert], more: usize, clock: f64, seen: &HashSet<u32>) -> Vec<El> {
    let mut out: Vec<El> = list
        .iter()
        .map(|a| {
            let since = (clock - a.since_s).max(0.0);
            brass::alert(&AlertSpec {
                icon: picture(a.what).into(),
                title: title(a),
                text: format!("{}.", a.text),
                age: age(since),
                urgency: urgency(a),
                go: format!("isle.alert.go:{}", a.id),
                dismiss: format!("isle.alert.hide:{}", a.id),
                fresh: since < FRESH_S,
                width: 400.0,
                age_id: age_id(a.id),
                settled: seen.contains(&a.id),
                ..Default::default()
            })
        })
        .collect();
    if more > 0 {
        out.push(
            El::text(format!(
                "and {more} more {}",
                if more == 1 { "alert" } else { "alerts" }
            ))
            .text_style(|t| t.size(14.0).italic().bold())
            .with_style(|s| {
                s.color(Colors::cream())
                    .padding([2.0, 12.0])
                    .shadow(engine_ui::style::Shadow::new(
                        engine_ui::Color::hexa(0x000000c0),
                        6.0,
                        1.0,
                    ))
            })
            .pick_ignore(),
        );
    }
    out
}

/// Rebuild the strips when what they show changed (the ages move every few refreshes).
pub fn refresh(world: &mut World) {
    if !world.contains_resource::<Alerts>() {
        return;
    }
    let clock = world.resource::<Economy>().clock_s;
    let (list, more) = shown(world);
    let x = left(world);
    let sig = format!(
        "{}|{more}",
        list.iter()
            .map(|a| format!(
                "{}:{}:{}:{}:{}",
                a.id,
                a.crew,
                a.text,
                urgency(a) as u8,
                clock - a.since_s < FRESH_S
            ))
            .collect::<Vec<_>>()
            .join(",")
    );
    let Some(node) = engine_ui::input::find_id(world, LIST_ID) else {
        return;
    };
    if world.resource::<Alerts>().left != x
        && let Some(mut n) = world.get_mut::<engine_ui::node::Node>(node)
    {
        n.style.left = Some(engine_ui::style::Val::Px(x));
        world.resource_mut::<Alerts>().left = x;
    }
    if world.resource::<Alerts>().sig != sig {
        let seen = world.resource::<Alerts>().seen.clone();
        let els = strips(&list, more, clock, &seen);
        let _ = engine_ui::el::replace_children(world, node, &els);
        let mut r = world.resource_mut::<Alerts>();
        r.sig = sig;
        r.seen = list.iter().map(|a| a.id).collect();
    }
    // How long ago, in place.
    for a in &list {
        let text = age((clock - a.since_s).max(0.0));
        if let Some(n) = engine_ui::input::find_id(world, &age_id(a.id))
            && let Some(mut t) = world.get_mut::<engine_ui::node::Text>(n)
            && t.text != text
        {
            t.text = text;
        }
    }
}

/// `isle.alert.*` buttons. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    if let Some(id) = a
        .strip_prefix("isle.alert.go:")
        .and_then(|s| s.parse::<u32>().ok())
    {
        go(world, id);
    } else if let Some(id) = a
        .strip_prefix("isle.alert.hide:")
        .and_then(|s| s.parse::<u32>().ok())
    {
        world.resource_mut::<Alerts>().hidden.insert(id);
        refresh(world);
    } else {
        return false;
    }
    true
}

/// Glide the view to alert `id`'s building and open its card.
fn go(world: &mut World, id: u32) {
    let Some(a) = crate::incidents::alerts(world)
        .into_iter()
        .find(|a| a.id == id)
    else {
        return;
    };
    super::map::look_at(world, DVec2::new(a.at[0], a.at[2]));
    if let Some(e) = crate::build::by_id(world, a.building) {
        world.resource_mut::<crate::tools::Hand>().selected = Some(e);
        super::show_card(world);
    }
}

/// `--hud-alert-go`: press "Go there" on the top alert (tour shots).
pub fn demo_go(world: &mut World) {
    if let Some(a) = shown(world).0.first() {
        go(world, a.id);
    }
}
