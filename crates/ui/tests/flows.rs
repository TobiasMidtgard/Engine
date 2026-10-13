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
