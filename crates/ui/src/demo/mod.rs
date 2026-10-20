//! The UI showcase: sample data (a player with a full bag, a merchant, a conversation, star
//! maps) and [`spawn_demo`], which the showcase world calls to place the UI test area.
//!
//! Every screen can then be opened from the console or by an AI agent:
//! `ui.show {"screen": "dialogue", "tree": "merchant_intro"}`, `{"screen": "shop"}`,
//! `{"screen": "inventory"}`, `{"screen": "map"}`, `{"screen": "journal"}`, `{"screen": "skills"}`, `{"screen": "crafting"}`,
//! `{"screen": "pause"}`.

use bevy_ecs::prelude::*;
use engine_core::math::DVec3;
use engine_core::prelude::Transform;
use serde_json::json;

use crate::data::UiData;
use crate::items::*;

pub mod art;
pub mod market;

/// Name of the demo player entity (owner of the demo inventory).
pub const PLAYER: &str = "Player";
/// Name of the demo merchant entity.
pub const MERCHANT: &str = "Merchant";

/// The demo player's inventory: a bit of everything, some equipped.
pub fn player_inventory(db: &ItemDatabase) -> Inventory {
    let mut inv = Inventory::with_capacity(40);
    inv.gold = 1_250;
    for (id, n) in [
        ("greatsword", 1),
        ("dagger", 1),
        ("bow", 1),
        ("pistol", 1),
        ("chest_armor", 1),
        ("boots", 1),
        ("cloak", 1),
        ("ring", 1),
        ("potion_health", 7),
        ("potion_mana", 4),
        ("potion_stamina", 3),
        ("elixir", 1),
        ("medkit", 2),
        ("bread", 6),
        ("fish", 3),
        ("herb", 23),
        ("mushroom_glow", 8),
        ("crystal_glow", 2),
        ("ore_iron", 14),
        ("ore_gold", 3),
        ("gem_ruby", 1),
        ("gem_emerald", 1),
        ("circuit", 2),
        ("fuel_cell", 2),
        ("grenade", 4),
        ("lockpick", 5),
        ("torch", 2),
        ("rope", 1),
        ("compass", 1),
        ("key", 1),
        ("map_scroll", 1),
        ("spellbook", 1),
    ] {
        inv.add(db, id, n);
    }
    for (slot, id) in [
        (EquipSlot::MainHand, "sword"),
        (EquipSlot::OffHand, "shield"),
        (EquipSlot::Head, "helmet"),
        (EquipSlot::Hands, "gloves"),
        (EquipSlot::Amulet, "amulet"),
    ] {
        inv.equipment.insert(slot, ItemStack::new(id, 1));
    }
    inv
}

/// The demo merchant: Old Brannoc of Hollow Reach.
pub fn merchant() -> Vendor {
    Vendor {
        name: "Old Brannoc".into(),
        portrait: "portrait:merchant".into(),
        greeting: "Mind the dust. Everything here has crossed at least three star systems.".into(),
        stock: [
            ("rifle", 1),
            ("staff", 1),
            ("hammer", 1),
            ("axe", 2),
            ("shield_small", 2),
            ("helmet", 1),
            ("chest_armor", 1),
            ("gloves", 3),
            ("potion_health", 0),
            ("potion_mana", 0),
            ("potion_stamina", 0),
            ("medkit", 6),
            ("bread", 0),
            ("meat", 0),
            ("fuel_cell", 4),
            ("grenade", 10),
            ("torch", 0),
            ("rope", 0),
            ("lockpick", 0),
            ("gem_sapphire", 2),
            ("crystal_glow", 3),
            ("circuit", 2),
        ]
        .into_iter()
        .map(|(id, n)| ItemStack::new(id, n))
        .collect(),
        markup: 1.0,
        buy_rate: 0.4,
        gold: 8_000,
    }
}

/// Sample values for HUD bindings.
pub fn player_data(data: &mut UiData) {
    data.set(
        "player",
        json!({
            "name": "Mara Voss",
            "level": 14,
            "health": 72.0, "health_max": 100.0,
            "shield": 38.0, "shield_max": 60.0,
            "stamina": 81.0, "stamina_max": 100.0,
            "mana": 45.0, "mana_max": 80.0,
            "xp": 6_350.0, "xp_max": 9_000.0,
            "gold": 1_250,
            "heading_deg": 32.0,
            "speed": 4.2
        }),
    );
    data.set(
        "quest",
        json!({
            "title": "The Silent Relay",
            "step": "Reach the relay tower on Veyra's northern ridge",
            "progress": 2, "progress_max": 4,
            "distance_m": 1_240.0
        }),
    );
    data.set(
        "weapon",
        json!({"name": "Ion Pistol", "icon": "pistol", "ammo": 12, "reserve": 36, "grenades": 4}),
    );
    data.set(
        "prompt",
        json!({"key": "E", "pad": "X", "verb": "Trade", "target": "Old Brannoc"}),
    );
    data.set(
        "abilities",
        json!([
            {"icon": "ability_slash", "key": "1", "cooldown": 0.0, "total": 2.0},
            {"icon": "ability_dash", "key": "2", "cooldown": 1.4, "total": 4.0},
            {"icon": "ability_fireball", "key": "3", "cooldown": 6.3, "total": 9.0},
            {"icon": "ability_frost", "key": "4", "cooldown": 0.0, "total": 8.0},
            {"icon": "ability_shield_bubble", "key": "5", "cooldown": 0.0, "total": 20.0},
            {"icon": "ability_time_slow", "key": "6", "cooldown": 12.0, "total": 30.0}
        ]),
    );
}

/// Spawn the demo player and merchant entities and fill [`UiData`] (idempotent).
pub fn setup_data(world: &mut World) {
    if !world.contains_resource::<ItemDatabase>() {
        world.insert_resource(ItemDatabase::default());
    }
    if !world.contains_resource::<UiData>() {
        world.insert_resource(UiData::default());
    }
    let db = world.resource::<ItemDatabase>().clone();
    let has =
        |world: &mut World, name: &str| engine_core::registry::find_by_name(world, name).is_ok();
    if !has(world, PLAYER) {
        world.spawn((Name::new(PLAYER), player_inventory(&db)));
    }
    if !has(world, MERCHANT) {
        world.spawn((Name::new(MERCHANT), merchant()));
    }
    player_data(&mut world.resource_mut::<UiData>());
    crate::dialogue::add_demo_trees(world);
    crate::map::add_demo_map(world);
    crate::quests::add_demo_quests(world);
    crate::wheel::add_demo_items(world);
    crate::skills::add_demo_tree(world);
    crate::crafting::add_demo_recipes(world);
}

/// Marks the UI test area's world objects.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct UiDemo;

/// Place the UI test area at `origin`: the demo data, the market street ([`market`]), and
/// world-anchored markers and nameplates over the area (a merchant's stall, a quest beacon, a
/// vista point). The screens themselves open through `ui.show` (the showcase tour does this
/// at each stop).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    commands.queue(move |world: &mut World| {
        setup_data(world);
        market::spawn_market(world, origin);
        // Over Old Brannoc's stall, the middle one, in view of the tour's UI stops.
        let (stall, _) = market::stall_place(2);
        crate::hud::spawn_world_markers(world, origin, stall + DVec3::Y * 3.4);
        world.spawn((UiDemo, Transform::from_translation(origin)));
    });
}
