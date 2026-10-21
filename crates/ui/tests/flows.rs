//! Screen flows driven the way a player (or an AI agent) would: mouse input, actions and the
//! `ui.*` / `shop.*` / `map.*` APIs. No rendering: layout and input run without a GPU.

use engine_core::math::Vec2;
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_input::{Binding, InputEvent, InputMap, KeyCode, MouseButton};
use engine_ui::items::{Inventory, ItemDatabase, Vendor};
use engine_ui::prelude::*;
use serde_json::{Value, json};

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1920;
        w.height = 1080;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((engine_input::InputPlugin, UiPlugin));
    engine_ui::demo::setup_data(&mut app.world);
    app.update_n(2);
    app
}

fn api(app: &mut App, name: &str, args: Value) -> Value {
    call_api(&mut app.world, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn open(app: &mut App) -> Vec<String> {
    let v = api(app, "ui.screens", json!({}));
    v["open"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap().to_string())
        .collect()
}

fn center(app: &mut App, id: &str) -> Vec2 {
    let e = engine_ui::input::find_id(&mut app.world, id).unwrap_or_else(|| panic!("no node {id}"));
    app.world.get::<ComputedNode>(e).unwrap().rect.center()
}

fn actions_since(app: &mut App, since: u64) -> Vec<String> {
    let v = api(app, "ui.events", json!({"since": since}));
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["action"].as_str().map(str::to_string))
        .collect()
}

fn next_event(app: &mut App) -> u64 {
    api(app, "ui.events", json!({"since": u64::MAX}))["next"]
        .as_u64()
        .unwrap()
}

fn input(app: &mut App, e: InputEvent) {
    app.world.write_message(e);
    app.update_n(2);
}

fn player(app: &mut App) -> Inventory {
    let mut q = app.world.query::<(&Name, &Inventory)>();
    q.iter(&app.world)
        .find(|(n, _)| n.as_str() == engine_ui::demo::PLAYER)
        .map(|(_, i)| i.clone())
        .unwrap()
}

#[test]
fn a_mouse_click_on_a_button_sends_its_action() {
    let mut app = app();
    let el = El::new()
        .class("panel")
        .with_style(|s| s.at(100.0, 100.0).width(300.0))
        .child(El::button("Go", "test.go").class("primary").id("go"));
    engine_ui::el::spawn(&mut app.world, None, &el).unwrap();
    app.update_n(10);
    let since = next_event(&mut app);
    let p = center(&mut app, "go");
    input(&mut app, InputEvent::CursorMoved { position: p });
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        },
    );
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: false,
        },
    );
    assert_eq!(actions_since(&mut app, since), vec!["test.go".to_string()]);
    // A press that slides off the button before release does nothing.
    let since = next_event(&mut app);
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        },
    );
    input(
        &mut app,
        InputEvent::CursorMoved {
            position: p + Vec2::new(900.0, 600.0),
        },
    );
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: false,
        },
    );
    assert!(actions_since(&mut app, since).is_empty());
}

#[test]
fn choosing_trade_in_a_conversation_opens_the_shop() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "dialogue", "tree": "merchant_intro", "node": "offer"}),
    );
    app.update_n(5);
    // Finish typing, then the choices appear.
    api(&mut app, "dialogue.next", json!({}));
    app.update_n(30);
    let st = api(&mut app, "dialogue.state", json!({}));
    assert_eq!(st["typing"], false);
    assert_eq!(st["choices"].as_array().unwrap().len(), 4);
    api(&mut app, "dialogue.choose", json!({"index": 0}));
    app.update_n(10);
    assert!(
        open(&mut app).contains(&"shop".to_string()),
        "{:?}",
        open(&mut app)
    );
    assert_eq!(api(&mut app, "dialogue.state", json!({}))["node"], "browse");
}

#[test]
fn skill_checks_branch_on_their_odds_and_the_log_keeps_what_was_said() {
    let mut app = app();
    let tree = json!({
        "start": "ask",
        "speakers": {
            "me": {"name": "Mara", "side": "left"},
            "g": {"name": "Guard", "side": "right"}
        },
        "nodes": {
            "ask": {"speaker": "g", "text": "Papers.", "choices": [
                {"text": "I'm expected.", "tag": "Lie", "chance": 1.0, "next": "ok", "fail": "no"},
                {"text": "You don't need to see them.", "tag": "Persuade", "chance": 0.0, "next": "ok", "fail": "no"},
                {"text": "Here.", "next": null}
            ]},
            "ok": {"speaker": "g", "text": "Go on, then.", "next": "ask"},
            "no": {"speaker": "g", "text": "Nice try.", "next": "ask"}
        }
    });
    api(
        &mut app,
        "ui.show",
        json!({"screen": "dialogue", "tree": tree, "speed": 0}),
    );
    app.update_n(5);
    let since = next_event(&mut app);
    let st = api(&mut app, "dialogue.state", json!({}));
    assert_eq!(st["choices"][0]["chance"], 1.0);
    // A sure thing passes...
    api(&mut app, "dialogue.choose", json!({"index": 0}));
    app.update_n(3);
    assert_eq!(api(&mut app, "dialogue.state", json!({}))["node"], "ok");
    assert!(actions_since(&mut app, since).contains(&"dialogue.check:lie:pass".to_string()));
    // ...and back at the question, that choice is marked as said.
    api(&mut app, "dialogue.next", json!({}));
    app.update_n(3);
    api(&mut app, "dialogue.next", json!({}));
    app.update_n(3);
    let st = api(&mut app, "dialogue.state", json!({}));
    assert_eq!(st["node"], "ask");
    assert_eq!(st["choices"][0]["seen"], true);
    assert_eq!(st["choices"][1]["seen"], false);
    // A long shot fails and takes the fail branch.
    api(&mut app, "dialogue.choose", json!({"index": 1}));
    app.update_n(3);
    assert_eq!(api(&mut app, "dialogue.state", json!({}))["node"], "no");
    assert!(actions_since(&mut app, since).contains(&"dialogue.check:persuade:fail".to_string()));
    // The log has every line and both replies, with the checks' results.
    let st = api(&mut app, "dialogue.state", json!({}));
    let log = st["log"].as_array().unwrap();
    let texts: Vec<&str> = log.iter().map(|l| l["text"].as_str().unwrap()).collect();
    assert_eq!(
        texts,
        [
            "Papers.",
            "I'm expected.",
            "Go on, then.",
            "Papers.",
            "You don't need to see them.",
            "Nice try."
        ]
    );
    assert_eq!(log[1]["speaker"], "me");
    assert_eq!(log[1]["check"], json!(["Lie", true]));
    assert_eq!(log[4]["check"], json!(["Persuade", false]));
    // The log opens over the text box and closes again.
    api(&mut app, "ui.click", json!({"id": "dlg_log_hint"}));
    app.update_n(3);
    assert_eq!(api(&mut app, "dialogue.state", json!({}))["log_open"], true);
    api(&mut app, "dialogue.log", json!({}));
    app.update_n(3);
    assert_eq!(
        api(&mut app, "dialogue.state", json!({}))["log_open"],
        false
    );
}

#[test]
fn buying_moves_gold_and_items() {
    let mut app = app();
    let before = player(&mut app);
    let price = {
        let db = app.world.resource::<ItemDatabase>().clone();
        let mut q = app.world.query::<&Vendor>();
        let v = q.iter(&app.world).next().unwrap();
        v.price(db.get("potion_health").unwrap())
    };
    api(
        &mut app,
        "shop.buy",
        json!({"item": "potion_health", "count": 2}),
    );
    let after = player(&mut app);
    assert_eq!(
        after.count_of("potion_health"),
        before.count_of("potion_health") + 2
    );
    assert_eq!(after.gold, before.gold - 2 * price);
    // Gold shown in the UI follows.
    app.update_n(2);
    let shown = api(&mut app, "ui.get_data", json!({"path": "player.gold"}));
    assert_eq!(
        shown.as_u64().or_else(|| shown["value"].as_u64()),
        Some(after.gold)
    );
}

#[test]
fn dragging_an_item_to_an_empty_slot_moves_it() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "inventory", "instant": true}),
    );
    app.update_n(30);
    let inv = player(&mut app);
    let from = inv.slots.iter().position(Option::is_some).unwrap();
    let to = inv.slots.iter().rposition(Option::is_none).unwrap();
    let item = inv.slots[from].clone().unwrap();
    let a = center(&mut app, &format!("inv_slot:{from}"));
    let b = center(&mut app, &format!("inv_slot:{to}"));
    input(&mut app, InputEvent::CursorMoved { position: a });
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        },
    );
    for k in 1..=8 {
        input(
            &mut app,
            InputEvent::CursorMoved {
                position: a.lerp(b, k as f32 / 8.0),
            },
        );
    }
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: false,
        },
    );
    app.update_n(5);
    let inv = player(&mut app);
    assert!(inv.slots[from].is_none(), "source slot emptied");
    assert_eq!(
        inv.slots[to].as_ref().map(|s| s.item.clone()),
        Some(item.item)
    );
}

#[test]
fn the_map_zooms_from_galaxy_to_a_planet_and_sets_waypoints() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "map", "zoom": 3.6, "instant": true}),
    );
    app.update_n(30);
    assert_eq!(api(&mut app, "map.state", json!({}))["level"], "Galaxy");
    api(
        &mut app,
        "map.view",
        json!({"focus": {"type": "planet", "id": "veyra"}, "zoom": 1.4}),
    );
    app.update_n(240);
    let st = api(&mut app, "map.state", json!({}));
    assert_eq!(st["level"], "Planet", "{st}");
    assert_eq!(st["planet"], "veyra");
    api(
        &mut app,
        "map.view",
        json!({"select": {"type": "poi", "id": "hollow_reach"}}),
    );
    engine_ui::input::dispatch(&mut app.world, "map.waypoint");
    app.update_n(3);
    let map = app.world.resource::<engine_ui::map::MapData>();
    let (_, poi) = map.poi("hollow_reach").unwrap();
    let wp = map.waypoint.clone().expect("waypoint set");
    assert_eq!(
        (wp.planet.as_str(), wp.lon, wp.lat),
        ("veyra", poi.lon, poi.lat)
    );
}

#[test]
fn pausing_stops_game_time_and_closing_restores_it() {
    let mut app = app();
    app.world.resource_mut::<Time>().set_base_scale(0.8);
    api(&mut app, "ui.show", json!({"screen": "pause"}));
    app.update_n(5);
    assert_eq!(app.world.resource::<Time>().base_scale(), 0.0);
    engine_ui::input::dispatch(&mut app.world, "ui.close");
    app.update_n(60);
    assert!(!open(&mut app).contains(&"pause".to_string()));
    assert!((app.world.resource::<Time>().base_scale() - 0.8).abs() < 1e-6);
}

#[test]
fn rebinding_a_key_in_settings_updates_the_input_map() {
    let mut app = app();
    let mut map = app.world.resource_mut::<InputMap>();
    if !map.actions.contains_key("jump") {
        map.bind("jump", Binding::Key(KeyCode::Space));
    }
    api(
        &mut app,
        "ui.show",
        json!({"screen": "settings", "tab": "controls", "instant": true}),
    );
    app.update_n(10);
    api(&mut app, "ui.click", json!({"id": "bind:jump"}));
    app.update_n(2);
    input(
        &mut app,
        InputEvent::Key {
            key: KeyCode::K,
            pressed: true,
        },
    );
    input(
        &mut app,
        InputEvent::Key {
            key: KeyCode::K,
            pressed: false,
        },
    );
    let map = app.world.resource::<InputMap>();
    let keys: Vec<_> = map.actions["jump"]
        .buttons
        .iter()
        .filter_map(|b| match b {
            Binding::Key(k) => Some(*k),
            _ => None,
        })
        .collect();
    assert_eq!(keys, vec![KeyCode::K]);
}

#[test]
fn markers_follow_a_world_shift() {
    let mut app = app();
    api(
        &mut app,
        "hud.marker",
        json!({"title": "Beacon", "position": [100.0, 5.0, -40.0]}),
    );
    app.update_n(2);
    app.world.write_message(engine_world::WorldShift::translate(
        engine_core::math::DVec3::new(-100.0, 0.0, 40.0),
    ));
    app.update_n(2);
    let mut q = app.world.query::<(&engine_ui::hud::Marker, &WorldAnchor)>();
    let (_, a) = q
        .iter(&app.world)
        .find(|(m, _)| m.title == "Beacon")
        .unwrap();
    assert!(
        (a.position - engine_core::math::DVec3::new(0.0, 5.0, 0.0)).length() < 1e-9,
        "{:?}",
        a.position
    );
}

#[test]
fn markers_sit_on_their_point_and_clamp_inside_the_screen() {
    use engine_core::math::{DVec3, Vec3};
    let mut app = app();
    app.world.spawn((
        engine_render::Camera::perspective(60.0),
        Transform::default().looking_at(DVec3::new(0.0, 0.0, -1.0), Vec3::Y),
    ));
    api(
        &mut app,
        "ui.show",
        json!({"screen": "hud", "instant": true}),
    );
    api(
        &mut app,
        "hud.marker",
        json!({"title": "Ahead", "position": [0.0, 0.0, -10.0]}),
    );
    api(
        &mut app,
        "hud.marker",
        json!({"title": "Behind", "position": [-50.0, 0.0, 400.0], "clamp": true}),
    );
    app.update_n(10);
    let rect = |app: &mut App, id: &str| {
        let e = engine_ui::input::find_id(&mut app.world, id).unwrap();
        app.world.get::<ComputedNode>(e).unwrap().rect
    };
    let ahead = rect(&mut app, "marker:Ahead");
    assert!(
        (ahead.center() - Vec2::new(960.0, 540.0)).length() < 2.0,
        "{ahead:?}"
    );
    assert!(ahead.h < 200.0, "sized to its content: {ahead:?}");
    let behind = rect(&mut app, "marker:Behind");
    assert!(behind.x >= 0.0 && behind.y >= 0.0, "{behind:?}");
    assert!(
        behind.x + behind.w <= 1920.0 && behind.y + behind.h <= 1080.0,
        "{behind:?}"
    );
}

#[test]
fn a_waypoint_marker_shows_the_distance_to_the_map_waypoint() {
    use engine_core::math::{DVec3, Vec3};
    use engine_ui::hud::{MarkerDistance, format_distance};
    let mut app = app();
    app.world.spawn((
        engine_render::Camera::perspective(60.0),
        Transform::default().looking_at(DVec3::new(0.0, 0.0, -1.0), Vec3::Y),
    ));
    api(
        &mut app,
        "ui.show",
        json!({"screen": "hud", "instant": true}),
    );
    api(
        &mut app,
        "hud.marker",
        json!({"title": "Beacon", "position": [0.0, 0.0, -10.0], "waypoint": true}),
    );
    app.update_n(4);
    let (far, _) = app
        .world
        .resource::<engine_ui::map::MapData>()
        .waypoint_nav()
        .expect("the demo waypoint is on the player's planet");
    let root = engine_ui::input::find_id(&mut app.world, "marker:Beacon").unwrap();
    let text = engine_ui::screens::subtree(&app.world, root)
        .into_iter()
        .find(|&e| app.world.get::<MarkerDistance>(e).is_some())
        .and_then(|e| app.world.get::<Text>(e))
        .map(|t| t.text.clone())
        .expect("a distance label");
    assert_eq!(text, format_distance(far as f32 as f64));
    assert!(far > 10_000.0, "far beyond the anchor 10 m away: {far}");
}

#[test]
fn shop_and_inventory_open_with_the_item_asked_for() {
    use engine_ui::inventory::{InventoryScreen, SlotRef};
    use engine_ui::shop::ShopScreen;
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "shop", "select": "hammer", "instant": true}),
    );
    app.update_n(2);
    let mut q = app.world.query::<&ShopScreen>();
    let st = q.single(&app.world).unwrap();
    let vendor = app.world.get::<Vendor>(st.vendor).unwrap();
    let i = st.selected.expect("a card is selected");
    assert_eq!(vendor.stock[i].item, "hammer");
    assert_eq!(st.shown, Some(i));
    api(&mut app, "ui.hide_all", json!({}));
    api(
        &mut app,
        "ui.show",
        json!({"screen": "inventory", "select": 3, "instant": true}),
    );
    app.update_n(2);
    let mut q = app.world.query::<&InventoryScreen>();
    let st = q.single(&app.world).unwrap();
    assert_eq!(st.selected, Some(SlotRef::Bag(3)));
}

fn quests(app: &mut App) -> engine_ui::quests::QuestLog {
    app.world.resource::<engine_ui::quests::QuestLog>().clone()
}

fn waypoint_label(app: &mut App) -> String {
    app.world
        .resource::<engine_ui::map::MapData>()
        .waypoint
        .as_ref()
        .map(|w| w.label.clone())
        .unwrap_or_default()
}

#[test]
fn tracking_a_quest_moves_the_hud_tracker_and_the_waypoint() {
    let mut app = app();
    let data = |app: &mut App, path: &str| api(app, "ui.get_data", json!({"path": path}));
    assert_eq!(data(&mut app, "quest.title"), "The Silent Relay");
    assert_eq!(data(&mut app, "quest.progress_max"), 4);
    assert_eq!(waypoint_label(&mut app), "Relay Tower");
    api(&mut app, "quest.track", json!({"id": "storm_salvage"}));
    app.update_n(2);
    assert_eq!(data(&mut app, "quest.title"), "Storm Salvage");
    assert_eq!(
        data(&mut app, "quest.step"),
        "Recover flight recorders from the Scar"
    );
    // A counted step shows its own count.
    assert_eq!(data(&mut app, "quest.progress"), 1);
    assert_eq!(data(&mut app, "quest.progress_max"), 4);
    assert_eq!(waypoint_label(&mut app), "The Scar");
    // Tracking nothing empties the tracker and leaves the waypoint where it was.
    api(&mut app, "quest.track", json!({"id": null}));
    app.update_n(2);
    assert_eq!(data(&mut app, "quest.title"), "");
    assert_eq!(waypoint_label(&mut app), "The Scar");
}

#[test]
fn the_journal_opens_on_the_tracked_quest_and_tracks_with_a_click() {
    use engine_ui::quests::JournalScreen;
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "journal", "instant": true}),
    );
    app.update_n(3);
    let selected = |app: &mut App| {
        let mut q = app.world.query::<&JournalScreen>();
        let st = q.single(&app.world).unwrap();
        (st.tab, st.selected.clone().unwrap_or_default())
    };
    assert_eq!(selected(&mut app), (0, "silent_relay".into()));
    // The tracked quest's place is on the banner map.
    let e = engine_ui::input::find_id(&mut app.world, "journal_map").expect("banner map");
    let view = app.world.get::<engine_ui::map::MapView>(e).unwrap();
    assert_eq!(
        view.selected,
        Some(engine_ui::map::MapItem::Poi("relay_tower".into()))
    );
    // The new quest loses its mark once looked at.
    assert!(quests(&mut app).get("lookout_log").unwrap().new);
    api(&mut app, "ui.click", json!({"id": "quest:lookout_log"}));
    app.update_n(2);
    assert_eq!(selected(&mut app), (0, "lookout_log".into()));
    assert!(!quests(&mut app).get("lookout_log").unwrap().new);
    api(&mut app, "ui.click", json!({"id": "journal_track"}));
    app.update_n(2);
    assert_eq!(quests(&mut app).tracked.as_deref(), Some("lookout_log"));
    assert_eq!(waypoint_label(&mut app), "Ridge Lookout");
    // Right click on a row tracks that one.
    api(
        &mut app,
        "ui.click",
        json!({"id": "quest:salt_run", "alt": true}),
    );
    app.update_n(2);
    assert_eq!(quests(&mut app).tracked.as_deref(), Some("salt_run"));
    // The finished tab opens on its first quest; Show on map opens the map over the journal.
    api(&mut app, "ui.click", json!({"id": "journal.tab:1"}));
    app.update_n(2);
    assert_eq!(selected(&mut app), (1, "landfall".into()));
    api(&mut app, "ui.click", json!({"id": "journal_show_map"}));
    app.update_n(3);
    let screens = open(&mut app);
    assert!(
        screens.contains(&"map".to_string()) && screens.contains(&"journal".to_string()),
        "{screens:?}"
    );
}

#[test]
fn finishing_the_last_step_completes_the_quest_and_pays_out() {
    use engine_ui::quests::QuestState;
    let mut app = app();
    let gold = player(&mut app).gold;
    let xp = api(&mut app, "ui.get_data", json!({"path": "player.xp"}))
        .as_f64()
        .unwrap();
    // One crystal more finishes the first step and shows the next one.
    let r = api(
        &mut app,
        "quest.progress",
        json!({"id": "glass_and_glow", "add": 1}),
    );
    assert_eq!(r["done"], true);
    let q = quests(&mut app).get("glass_and_glow").unwrap().clone();
    assert_eq!(
        (q.state, q.current(), q.visible()),
        (QuestState::Active, Some(1), 2)
    );
    // The salt run has one step: three medkits.
    api(
        &mut app,
        "quest.progress",
        json!({"id": "salt_run", "add": 3}),
    );
    app.update_n(2);
    assert_eq!(
        quests(&mut app).get("salt_run").unwrap().state,
        QuestState::Completed
    );
    assert_eq!(player(&mut app).gold, gold + 240);
    let xp2 = api(&mut app, "ui.get_data", json!({"path": "player.xp"}))
        .as_f64()
        .unwrap();
    assert_eq!(xp2, xp + 180.0);
    // Completing the tracked quest hands tracking to the next main quest... there is none
    // left, so it goes to the first active one.
    api(&mut app, "quest.complete", json!({"id": "silent_relay"}));
    app.update_n(2);
    let log = quests(&mut app);
    assert_eq!(
        log.get("silent_relay").unwrap().state,
        QuestState::Completed
    );
    assert_eq!(log.tracked.as_deref(), Some("glass_and_glow"));
}

#[test]
fn a_dialogue_choice_tracks_a_quest() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "dialogue", "tree": "quill_intro", "node": "c", "speed": 0}),
    );
    app.update_n(5);
    api(&mut app, "dialogue.next", json!({}));
    app.update_n(10);
    api(&mut app, "dialogue.choose", json!({"index": 1}));
    app.update_n(5);
    assert_eq!(quests(&mut app).tracked.as_deref(), Some("glass_and_glow"));
    assert_eq!(waypoint_label(&mut app), "Glass Caves");
    assert_eq!(
        api(&mut app, "dialogue.state", json!({}))["node"],
        "crystals"
    );
}

#[test]
fn short_labels_stay_on_one_line() {
    let mut app = app();
    // A scaled UI (900 px tall) lays out at fractional sizes.
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1600;
        w.height = 900;
    }
    app.update_n(2);
    api(
        &mut app,
        "ui.show",
        json!({"screen": "inventory", "select": "greatsword", "instant": true}),
    );
    app.update_n(4);
    let mut q = app.world.query::<(Entity, &Text)>();
    let chips: Vec<Entity> = q
        .iter(&app.world)
        .filter(|(_, t)| t.text == "Main hand" || t.text == "Weapon")
        .map(|(e, _)| e)
        .collect();
    assert!(!chips.is_empty());
    let layouts = app.world.resource::<engine_ui::text::TextLayouts>();
    for e in chips {
        let lines = layouts.entries[&e].buffer.layout_runs().count();
        assert_eq!(lines, 1, "a short label wrapped");
    }
}

fn dilation(app: &mut App) -> f32 {
    app.world
        .resource::<engine_core::time::TimeDilation>()
        .factor()
}

fn data_str(app: &mut App, path: &str) -> String {
    let d = app.world.resource::<engine_ui::data::UiData>();
    d.get(path)
        .map(|v| {
            v.as_str()
                .map(String::from)
                .unwrap_or_else(|| v.to_string())
        })
        .unwrap_or_default()
}

fn wheel_hovered(app: &mut App) -> Option<usize> {
    let mut q = app.world.query::<&engine_ui::wheel::WheelView>();
    q.iter(&app.world).next().and_then(|v| v.hovered)
}

fn press(app: &mut App, key: KeyCode) {
    input(app, InputEvent::Key { key, pressed: true });
    input(
        app,
        InputEvent::Key {
            key,
            pressed: false,
        },
    );
}

#[test]
fn the_wheel_slows_time_points_with_the_cursor_and_equips_on_click() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "hud", "instant": true}),
    );
    api(&mut app, "ui.show", json!({"screen": "wheel"}));
    app.update_n(40);
    // The world slows while it is open; the HUD stays up underneath.
    assert!(dilation(&mut app) < 0.3, "{}", dilation(&mut app));
    let hud = open(&mut app).contains(&"hud".to_string());
    assert!(hud);
    // The pistol is in hand, so it starts hovered.
    assert_eq!(wheel_hovered(&mut app), Some(0));
    // Point to the right of the middle: slot 2 (the greatsword, clockwise from the top).
    let c = center(&mut app, "wheel_canvas");
    input(
        &mut app,
        InputEvent::CursorMoved {
            position: c + Vec2::new(220.0, 0.0),
        },
    );
    assert_eq!(wheel_hovered(&mut app), Some(2));
    let since = next_event(&mut app);
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        },
    );
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: false,
        },
    );
    assert!(
        actions_since(&mut app, since).contains(&"weapon.equip:greatsword".to_string()),
        "{:?}",
        actions_since(&mut app, since)
    );
    assert_eq!(data_str(&mut app, "weapon.icon"), "greatsword");
    assert_eq!(data_str(&mut app, "wheel.current"), "greatsword");
    app.update_n(60);
    assert!(!open(&mut app).contains(&"wheel".to_string()));
    // Time runs again once it has closed.
    assert!((dilation(&mut app) - 1.0).abs() < 1e-3);
    // The HUD shows the new weapon.
    let e = engine_ui::input::find_id(&mut app.world, "hud_weapon_icon").unwrap();
    assert_eq!(app.world.get::<Icon>(e).unwrap().name, "greatsword");
}

#[test]
fn number_keys_pick_a_slot_and_unavailable_slots_stay_shut() {
    let mut app = app();
    api(&mut app, "ui.show", json!({"screen": "wheel"}));
    app.update_n(20);
    // Slot 7 is Time Slow, cooling down: nothing happens and the wheel stays open.
    press(&mut app, KeyCode::Digit7);
    app.update_n(5);
    assert!(open(&mut app).contains(&"wheel".to_string()));
    assert_eq!(data_str(&mut app, "wheel.current"), "pistol");
    // Slot 2 is the rail rifle.
    press(&mut app, KeyCode::Digit2);
    app.update_n(60);
    assert!(!open(&mut app).contains(&"wheel".to_string()));
    assert_eq!(data_str(&mut app, "weapon.icon"), "rifle");
    assert_eq!(data_str(&mut app, "weapon.ammo"), "4");
}

#[test]
fn a_held_wheel_picks_on_release_and_cancels_if_the_pointer_never_moved() {
    let mut app = app();
    // Held with Tab, let go without pointing: closes, nothing changes.
    input(
        &mut app,
        InputEvent::Key {
            key: KeyCode::Tab,
            pressed: true,
        },
    );
    api(
        &mut app,
        "ui.show",
        json!({"screen": "wheel", "hold": "Tab"}),
    );
    app.update_n(20);
    input(
        &mut app,
        InputEvent::Key {
            key: KeyCode::Tab,
            pressed: false,
        },
    );
    app.update_n(60);
    assert!(!open(&mut app).contains(&"wheel".to_string()));
    assert_eq!(data_str(&mut app, "wheel.selected"), "");
    // Held again with the cursor locked: raw motion straight down aims at the grenades
    // (slot 5 of 8, clockwise from the top).
    input(
        &mut app,
        InputEvent::Key {
            key: KeyCode::Tab,
            pressed: true,
        },
    );
    api(
        &mut app,
        "ui.show",
        json!({"screen": "wheel", "hold": "Tab"}),
    );
    app.update_n(20);
    input(
        &mut app,
        InputEvent::MouseMotion {
            delta: Vec2::new(0.0, 160.0),
        },
    );
    assert_eq!(wheel_hovered(&mut app), Some(4));
    input(
        &mut app,
        InputEvent::Key {
            key: KeyCode::Tab,
            pressed: false,
        },
    );
    app.update_n(60);
    assert!(!open(&mut app).contains(&"wheel".to_string()));
    assert_eq!(data_str(&mut app, "wheel.selected"), "grenade");
    // Grenades are thrown, not held: the pistol stays in hand.
    assert_eq!(data_str(&mut app, "wheel.current"), "pistol");
}

#[test]
fn the_click_that_picks_from_the_wheel_does_not_also_attack() {
    let mut app = app();
    app.world
        .resource_mut::<InputMap>()
        .bind("attack", Binding::Mouse(MouseButton::Left));
    api(&mut app, "ui.show", json!({"screen": "wheel"}));
    app.update_n(20);
    let c = center(&mut app, "wheel_canvas");
    input(
        &mut app,
        InputEvent::CursorMoved {
            position: c + Vec2::new(0.0, -220.0),
        },
    );
    // Press, hold while the wheel closes, then let go: the game never sees an attack.
    app.world.write_message(InputEvent::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    });
    let mut attacked = false;
    for _ in 0..30 {
        app.update();
        let a = app
            .world
            .resource::<engine_input::ActionState>()
            .get("attack");
        attacked |= a.just_pressed || a.pressed_since_tick;
    }
    assert!(!open(&mut app).contains(&"wheel".to_string()));
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: false,
        },
    );
    let a = app
        .world
        .resource::<engine_input::ActionState>()
        .get("attack");
    attacked |= a.just_pressed || a.pressed_since_tick;
    assert!(!attacked);
    // The next click attacks as usual.
    input(
        &mut app,
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        },
    );
    assert!(
        app.world
            .resource::<engine_input::ActionState>()
            .get("attack")
            .pressed
    );
}

#[test]
fn letting_go_over_an_unavailable_slot_closes_the_wheel_and_restores_time() {
    let mut app = app();
    input(
        &mut app,
        InputEvent::Key {
            key: KeyCode::Tab,
            pressed: true,
        },
    );
    api(
        &mut app,
        "ui.show",
        json!({"screen": "wheel", "hold": "Tab"}),
    );
    app.update_n(20);
    // Left points at Time Slow, which is cooling down.
    input(
        &mut app,
        InputEvent::MouseMotion {
            delta: Vec2::new(-160.0, 0.0),
        },
    );
    assert_eq!(wheel_hovered(&mut app), Some(6));
    input(
        &mut app,
        InputEvent::Key {
            key: KeyCode::Tab,
            pressed: false,
        },
    );
    app.update_n(60);
    assert!(!open(&mut app).contains(&"wheel".to_string()));
    assert_eq!(data_str(&mut app, "wheel.selected"), "");
    assert!((dilation(&mut app) - 1.0).abs() < 1e-3);
}

#[test]
fn showing_the_wheel_again_without_slow_motion_lets_time_run() {
    let mut app = app();
    api(&mut app, "ui.show", json!({"screen": "wheel"}));
    app.update_n(30);
    assert!(dilation(&mut app) < 0.3);
    api(&mut app, "ui.show", json!({"screen": "wheel", "slow": 1.0}));
    app.update_n(60);
    assert!(open(&mut app).contains(&"wheel".to_string()));
    assert!((dilation(&mut app) - 1.0).abs() < 1e-3);
}

fn skills(app: &mut App) -> engine_ui::skills::SkillTree {
    app.world.resource::<engine_ui::skills::SkillTree>().clone()
}

fn skill_selected(app: &mut App) -> String {
    let mut q = app.world.query::<&engine_ui::skills::SkillsScreen>();
    q.single(&app.world)
        .unwrap()
        .selected
        .clone()
        .unwrap_or_default()
}

#[test]
fn the_skill_tree_opens_from_pause_and_learns_with_a_point() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "pause", "instant": true}),
    );
    app.update_n(2);
    api(&mut app, "ui.click", json!({"id": "pause_skills"}));
    app.update_n(3);
    assert!(open(&mut app).contains(&"skills".to_string()));
    // It opens on the first skill with a rank to learn; a click picks another.
    assert_eq!(skill_selected(&mut app), "c_momentum");
    api(&mut app, "ui.click", json!({"id": "skill:c_cleave"}));
    app.update_n(2);
    assert_eq!(skill_selected(&mut app), "c_cleave");
    let before = skills(&mut app).points;
    let since = next_event(&mut app);
    api(&mut app, "ui.click", json!({"id": "skills_learn"}));
    app.update_n(2);
    let tree = skills(&mut app);
    assert_eq!(tree.get("c_cleave").unwrap().rank, 1);
    assert_eq!(tree.points, before - 1);
    assert!(actions_since(&mut app, since).contains(&"skill.learned:c_cleave".to_string()));
    let data = app.world.resource::<UiData>();
    assert_eq!(data.get_f64("skills.c_cleave"), Some(1.0));
    assert_eq!(
        data.get_f64("player.skill_points"),
        Some((before - 1) as f64)
    );
    // Space ranks it up again.
    press(&mut app, KeyCode::Space);
    assert_eq!(skills(&mut app).get("c_cleave").unwrap().rank, 2);
}

#[test]
fn a_locked_skill_refuses_and_reset_refunds_after_a_confirm() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "skills", "instant": true}),
    );
    app.update_n(3);
    // Whirlwind needs Cleave first; Thousand Cuts needs level 20.
    api(&mut app, "ui.click", json!({"id": "skill:c_whirl"}));
    app.update_n(2);
    assert_eq!(skill_selected(&mut app), "c_whirl");
    let points = skills(&mut app).points;
    // Its Learn button is disabled.
    assert!(call_api(&mut app.world, "ui.click", json!({"id": "skills_learn"})).is_err());
    press(&mut app, KeyCode::Space);
    assert_eq!(skills(&mut app).get("c_whirl").unwrap().rank, 0);
    assert_eq!(skills(&mut app).points, points);
    assert!(engine_ui::skills::learn(&mut app.world, "c_ult").is_err());
    // Reset asks first, then gives every point back.
    let spent = skills(&mut app).spent();
    assert!(spent > 0);
    api(&mut app, "ui.click", json!({"id": "skills_reset"}));
    app.update_n(3);
    assert!(open(&mut app).contains(&"confirm".to_string()));
    assert_eq!(skills(&mut app).spent(), spent);
    api(&mut app, "ui.click", json!({"id": "confirm_ok"}));
    app.update_n(3);
    let tree = skills(&mut app);
    assert_eq!(tree.spent(), 0);
    assert_eq!(tree.points, points + spent);
    let screens = open(&mut app);
    assert!(
        screens.contains(&"skills".to_string()) && !screens.contains(&"confirm".to_string()),
        "{screens:?}"
    );
}

#[test]
fn the_skills_api_learns_and_grants_points() {
    let mut app = app();
    let v = api(&mut app, "skills.grant", json!({"points": 2}));
    assert_eq!(v["points"], 5, "{v}");
    api(&mut app, "skills.learn", json!({"id": "t_overclock"}));
    assert_eq!(skills(&mut app).get("t_overclock").unwrap().rank, 1);
    assert!(call_api(&mut app.world, "skills.learn", json!({"id": "t_ult"})).is_err());
    let tree = api(&mut app, "skills.tree", json!({}));
    assert!(tree.to_string().contains("t_overclock"));
}

fn crafting(app: &mut App) -> engine_ui::crafting::CraftingScreen {
    let mut q = app.world.query::<&engine_ui::crafting::CraftingScreen>();
    q.single(&app.world).unwrap().clone()
}

#[test]
fn crafting_three_brews_takes_time_then_moves_items() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "crafting", "instant": true}),
    );
    app.update_n(3);
    // The alchemy bench opens on the first brew the bag can make.
    assert_eq!(
        crafting(&mut app).selected.as_deref(),
        Some("r_potion_health")
    );
    let (herbs, brews) = (
        player(&mut app).count_of("herb"),
        player(&mut app).count_of("potion_health"),
    );
    api(&mut app, "ui.click", json!({"id": "craft_more"}));
    app.update_n(2);
    api(&mut app, "ui.click", json!({"id": "craft_more"}));
    app.update_n(2);
    assert_eq!(crafting(&mut app).quantity, 3);
    let since = next_event(&mut app);
    api(&mut app, "ui.click", json!({"id": "craft_go"}));
    app.update_n(2);
    // Nothing leaves the bag until the craft finishes.
    assert!(crafting(&mut app).job.is_some());
    assert_eq!(player(&mut app).count_of("herb"), herbs);
    app.update_n(160);
    assert!(crafting(&mut app).job.is_none());
    assert_eq!(player(&mut app).count_of("herb"), herbs - 9);
    assert_eq!(player(&mut app).count_of("potion_health"), brews + 6);
    assert!(actions_since(&mut app, since).contains(&"craft.made:r_potion_health".to_string()));
}

#[test]
fn a_short_or_unknown_recipe_cannot_be_crafted_until_learned() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "crafting", "station": "smithing", "instant": true}),
    );
    app.update_n(3);
    assert_eq!(
        crafting(&mut app).station,
        engine_ui::crafting::Station::Smithing
    );
    // The sword needs an Oak Log the bag doesn't have.
    api(&mut app, "ui.click", json!({"id": "recipe:r_sword"}));
    app.update_n(2);
    assert_eq!(crafting(&mut app).selected.as_deref(), Some("r_sword"));
    assert!(call_api(&mut app.world, "ui.click", json!({"id": "craft_go"})).is_err());
    press(&mut app, KeyCode::Space);
    app.update_n(5);
    assert!(crafting(&mut app).job.is_none());
    // Tinkering, one station along: the rail rifle is unknown until its schematic is learned.
    api(&mut app, "ui.click", json!({"id": "craft.station:2"}));
    app.update_n(2);
    assert_eq!(
        crafting(&mut app).station,
        engine_ui::crafting::Station::Tinkering
    );
    assert!(call_api(&mut app.world, "craft.make", json!({"id": "r_rifle"})).is_err());
    let v = api(&mut app, "craft.learn", json!({"id": "r_rifle"}));
    assert_eq!(v["learned"], true);
    // Learned, the bag holds just enough for one.
    api(&mut app, "craft.make", json!({"id": "r_rifle"}));
    assert_eq!(player(&mut app).count_of("rifle"), 1);
    let err = call_api(&mut app.world, "craft.make", json!({"id": "r_rifle"})).unwrap_err();
    assert!(err.contains("Missing"), "{err}");
    // The API crafts at once.
    let picks = player(&mut app).count_of("lockpick");
    api(
        &mut app,
        "craft.make",
        json!({"id": "r_lockpick", "count": 2}),
    );
    assert_eq!(player(&mut app).count_of("lockpick"), picks + 4);
}

fn saves(app: &mut App) -> engine_ui::saves::SaveSlots {
    app.world.resource::<engine_ui::saves::SaveSlots>().clone()
}

fn saves_selected(app: &mut App) -> String {
    let mut q = app.world.query::<&engine_ui::saves::SavesScreen>();
    q.single(&app.world)
        .unwrap()
        .selected
        .clone()
        .unwrap_or_default()
}

#[test]
fn loading_a_save_from_pause_asks_first_then_sends_game_load() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "pause", "instant": true}),
    );
    app.update_n(2);
    api(&mut app, "ui.click", json!({"id": "pause_load"}));
    app.update_n(3);
    assert!(open(&mut app).contains(&"saves".to_string()));
    // Newest first: the autosave.
    assert_eq!(saves_selected(&mut app), "auto");
    api(&mut app, "ui.click", json!({"id": "save:save_3"}));
    app.update_n(2);
    assert_eq!(saves_selected(&mut app), "save_3");
    let since = next_event(&mut app);
    api(&mut app, "ui.click", json!({"id": "saves_go"}));
    app.update_n(3);
    assert!(open(&mut app).contains(&"confirm".to_string()));
    assert!(!actions_since(&mut app, since).contains(&"game.load:save_3".to_string()));
    api(&mut app, "ui.click", json!({"id": "confirm_ok"}));
    app.update_n(30);
    assert!(actions_since(&mut app, since).contains(&"game.load:save_3".to_string()));
    let screens = open(&mut app);
    assert!(
        !screens.contains(&"saves".to_string()) && !screens.contains(&"pause".to_string()),
        "{screens:?}"
    );
}

#[test]
fn saving_new_over_and_deleting_slots() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "saves", "mode": "save", "instant": true}),
    );
    app.update_n(3);
    let n = saves(&mut app).slots.len();
    // Save mode opens on a new save; Save here writes the game as it is now.
    let since = next_event(&mut app);
    api(&mut app, "ui.click", json!({"id": "saves_go"}));
    app.update_n(3);
    let s = saves(&mut app);
    assert_eq!(s.slots.len(), n + 1);
    let new = s.get("save_5").expect("a new slot");
    assert_eq!(new.location, "Hollow Reach");
    assert_eq!(new.chapter, "The Silent Relay");
    assert_eq!(new.playtime_s, 45_840);
    assert!(actions_since(&mut app, since).contains(&"game.save:save_5".to_string()));
    // Saving over a named slot keeps its name and asks first.
    api(&mut app, "ui.click", json!({"id": "save:save_4"}));
    app.update_n(2);
    api(&mut app, "ui.click", json!({"id": "saves_go"}));
    app.update_n(3);
    assert_eq!(
        saves(&mut app).get("save_4").unwrap().location,
        "Frostspine Pass"
    );
    api(&mut app, "ui.click", json!({"id": "confirm_ok"}));
    app.update_n(3);
    let s4 = saves(&mut app).get("save_4").cloned().unwrap();
    assert_eq!(
        (s4.location.as_str(), s4.name.as_str()),
        ("Hollow Reach", "Summit")
    );
    // Delete, after a confirm.
    api(&mut app, "ui.click", json!({"id": "save:save_2"}));
    app.update_n(2);
    api(&mut app, "ui.click", json!({"id": "saves_delete"}));
    app.update_n(3);
    api(&mut app, "ui.click", json!({"id": "confirm_ok"}));
    app.update_n(3);
    assert!(saves(&mut app).get("save_2").is_none());
    assert!(actions_since(&mut app, since).contains(&"game.delete_save:save_2".to_string()));
    // The deleted slot's selection moves on, and the screen stays open.
    assert_ne!(saves_selected(&mut app), "save_2");
    assert!(open(&mut app).contains(&"saves".to_string()));
}

#[test]
fn the_title_screen_continues_the_newest_save_and_opens_load_and_quit() {
    let mut app = app();
    api(
        &mut app,
        "ui.show",
        json!({"screen": "title", "instant": true}),
    );
    app.update_n(3);
    // Esc doesn't close the title screen.
    press(&mut app, KeyCode::Escape);
    app.update_n(3);
    assert!(open(&mut app).contains(&"title".to_string()));
    // Load Game opens the saves screen over it; closing it comes back.
    api(&mut app, "ui.click", json!({"id": "title_load"}));
    app.update_n(3);
    assert!(open(&mut app).contains(&"saves".to_string()));
    api(
        &mut app,
        "ui.hide",
        json!({"screen": "saves", "instant": true}),
    );
    app.update_n(3);
    // Quit asks first.
    let since = next_event(&mut app);
    api(&mut app, "ui.click", json!({"id": "title_quit"}));
    app.update_n(3);
    assert!(open(&mut app).contains(&"confirm".to_string()));
    assert!(!actions_since(&mut app, since).contains(&"game.exit".to_string()));
    api(&mut app, "ui.click", json!({"id": "confirm_cancel"}));
    app.update_n(30);
    // Continue loads the newest save (the autosave) and leaves the title screen.
    api(&mut app, "ui.click", json!({"id": "title_continue"}));
    app.update_n(30);
    assert!(actions_since(&mut app, since).contains(&"game.load:auto".to_string()));
    assert!(!open(&mut app).contains(&"title".to_string()));
    // New Game from a fresh title screen.
    api(
        &mut app,
        "ui.show",
        json!({"screen": "title", "instant": true}),
    );
    app.update_n(3);
    api(&mut app, "ui.click", json!({"id": "title_new"}));
    app.update_n(30);
    assert!(actions_since(&mut app, since).contains(&"game.new".to_string()));
    assert!(!open(&mut app).contains(&"title".to_string()));
    // ...and opens the loading screen for Veyra.
    assert!(open(&mut app).contains(&"loading".to_string()));
}

#[test]
fn the_loading_screen_follows_the_game_and_waits_to_be_dismissed() {
    let mut app = app();
    api(
        &mut app,
        "ui.set_data",
        json!({"path": "loading.progress", "value": 0.2}),
    );
    api(
        &mut app,
        "ui.show",
        json!({"screen": "loading", "planet": "kell", "place": "Wreck Field", "progress": "loading.progress", "instant": true}),
    );
    app.update_n(30);
    let shown = |app: &mut App| {
        let mut q = app.world.query::<&engine_ui::loading::LoadingView>();
        let v = q.iter(&app.world).next().expect("view").clone();
        (v.progress, v.shown, v.ready())
    };
    let (p, s, ready) = shown(&mut app);
    assert!((p - 0.2).abs() < 1e-4 && (s - 0.2).abs() < 0.01 && !ready);
    assert_eq!(engine_ui::loading::stage(p), "Streaming terrain");
    // Not done yet: a click does nothing.
    let since = next_event(&mut app);
    api(&mut app, "ui.click", json!({"id": "loading_backdrop"}));
    app.update_n(3);
    assert!(open(&mut app).contains(&"loading".to_string()));
    // Progress never runs backwards, even if the game writes a smaller value.
    api(
        &mut app,
        "ui.set_data",
        json!({"path": "loading.progress", "value": 0.1}),
    );
    app.update_n(3);
    assert!((shown(&mut app).0 - 0.2).abs() < 1e-4);
    // Done: the bar eases full, then the screen waits for a click.
    api(
        &mut app,
        "ui.set_data",
        json!({"path": "loading.progress", "value": 1.0}),
    );
    app.update_n(90);
    assert!(shown(&mut app).2);
    assert!(open(&mut app).contains(&"loading".to_string()));
    api(&mut app, "ui.click", json!({"id": "loading_backdrop"}));
    app.update_n(30);
    assert!(!open(&mut app).contains(&"loading".to_string()));
    assert!(actions_since(&mut app, since).contains(&"game.loading_done".to_string()));
    // A simulated load with hold off closes by itself.
    api(
        &mut app,
        "ui.show",
        json!({"screen": "loading", "simulate_s": 0.5, "hold": false, "instant": true}),
    );
    // Half a second of loading, the bar easing full, a short beat and the close.
    app.update_n(150);
    assert!(!open(&mut app).contains(&"loading".to_string()));
    // An unknown planet is an error, not an empty screen.
    let r = call_api(
        &mut app.world,
        "ui.show",
        json!({"screen": "loading", "planet": "nowhere"}),
    );
    assert!(r.is_err());
}
