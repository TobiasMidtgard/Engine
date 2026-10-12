#![doc = include_str!("../README.md")]

pub mod api;
pub mod atlas;
pub mod color;
pub mod data;
pub mod demo;
pub mod dialogue;
pub mod draw;
pub mod el;
pub mod feature;
pub mod hud;
pub mod icons;
pub mod input;
pub mod inventory;
pub mod items;
pub mod layout;
pub mod map;
pub mod menus;
pub mod node;
pub mod screens;
pub mod shop;
pub mod style;
pub mod text;
pub mod theme;
pub mod toast;
pub mod widgets;

pub use color::Color;
pub use data::UiData;
pub use draw::{DrawCtx, Painters};
pub use el::El;
pub use feature::UiFeature;
pub use icons::IconLibrary;
pub use input::{UiEvent, UiInputCapture, UiInputState};
pub use layout::{UiSettings, UiViewport};
pub use node::*;
pub use screens::{ScreenRegistry, hide, hide_all, show};
pub use style::{Fill, Shadow, Style, StylePatch, Val, pct};
pub use theme::{Palette, Theme};

use engine_core::prelude::*;
use engine_render::RenderAppExt;

/// Everything needed to build UI.
pub mod prelude {
    pub use crate::color::Color;
    pub use crate::data::UiData;
    pub use crate::el::El;
    pub use crate::input::{UiEvent, UiInputCapture};
    pub use crate::layout::UiSettings;
    pub use crate::node::*;
    pub use crate::screens::ScreenRegistry;
    pub use crate::style::*;
    pub use crate::theme::Theme;
    pub use crate::{UiPlugin, hide, show};
}

/// The UI: layout, text, theming, input, screens, the `ui.*` API and the render feature.
///
/// Works without a GPU (layout, input and events run; nothing is drawn) and without the
/// input plugin (the UI is then driven only through the API).
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Theme>()
            .init_resource::<UiSettings>()
            .init_resource::<UiViewport>()
            .init_resource::<UiData>()
            .init_resource::<text::Fonts>()
            .init_resource::<text::TextLayouts>()
            .init_resource::<draw::UiAtlases>()
            .init_resource::<IconLibrary>()
            .init_resource::<draw::DrawList>()
            .init_resource::<layout::UiDrawOrder>()
            .init_resource::<layout::UiLayout>()
            .init_resource::<Painters>()
            .init_resource::<ScreenRegistry>()
            .init_resource::<UiInputState>()
            .init_resource::<UiInputCapture>()
            .init_resource::<input::UiEventLog>()
            .add_message::<UiEvent>();

        register_types(app);

        app.world
            .resource_mut::<ScreenRegistry>()
            .add(
                "layout",
                "Any UI tree from JSON in `layout` (give it a `key` to open several).",
                r#"{"key": "quest_log", "modal": false, "layout": {"class": "panel", "style": {"width": 320}, "children": [{"text": "Find the relay", "class": "heading"}]}}"#,
                screens::build_layout,
            );

        {
            let mut painters = app.world.resource_mut::<Painters>();
            widgets::add_painters(&mut painters);
            hud::add_painters(&mut painters);
            map::add_painters(&mut painters);
        }
        app.init_resource::<items::ItemDatabase>()
            .init_resource::<dialogue::DialogueLibrary>()
            .init_resource::<map::MapData>()
            .init_resource::<menus::PauseState>();
        {
            let mut screens = app.world.resource_mut::<ScreenRegistry>();
            screens.add(
                "inventory",
                "The player's bag, equipment and item details, with drag and drop. `owner` names the entity with the Inventory (default: Player).",
                r#"{"owner": "Player", "tab": 0}"#,
                inventory::build_screen,
            );
            screens.add(
                "shop",
                "Buy from and sell to a merchant. `vendor` names the entity with the Vendor, `owner` the buyer (default: Player), `mode` \"buy\" or \"sell\".",
                r#"{"vendor": "Merchant", "mode": "buy"}"#,
                shop::build_screen,
            );
            screens.add(
                "dialogue",
                "A two-person conversation. `tree` is a DialogueLibrary id or a whole tree; `node` starts elsewhere; `speed` is characters per second (0 = instant).",
                r#"{"tree": "merchant_intro"}"#,
                dialogue::build_screen,
            );
            screens.add(
                "hud",
                "The in-game HUD bound to UiData (player.*, quest.*, abilities, weapon.*, prompt.*, location.*) and MapData. `portrait` names the player's portrait icon.",
                r#"{"portrait": "portrait:pilot"}"#,
                hud::build_screen,
            );
            screens.add(
                "map",
                "The star map: zoom 0-1 surface, 1-2 planet globe, 2-3 star system, 3-4 galaxy. `system`/`planet` pick what the inner levels show; `select` an item ({\"type\": \"poi\", \"id\": ...}).",
                r#"{"zoom": 1.4}"#,
                map::build_screen,
            );
            screens.add(
                "pause",
                "The pause menu (stops game time while open): resume, inventory, map, settings, save, quit.",
                "{}",
                menus::build_pause,
            );
            screens.add(
                "settings",
                "Settings: gameplay, video (RenderSettings), audio, controls (InputMap, rebinding) and interface (UiSettings, accent color). `tab` 0-4 or a name.",
                r#"{"tab": "video"}"#,
                menus::build_settings,
            );
            screens.add(
                "confirm",
                "A yes/no dialog: title, text, confirm (label), cancel (label), action (sent on confirm), danger.",
                r#"{"title": "Quit to title?", "text": "Unsaved progress will be lost.", "confirm": "Quit", "action": "game.quit", "danger": true}"#,
                menus::build_confirm,
            );
        }

        app.add_systems(
            PreUpdate,
            input::ui_input_system.after(engine_input::update_actions_system),
        )
        .add_systems(
            Update,
            (
                inventory::inventory_screen_system,
                shop::shop_screen_system,
                dialogue::dialogue_system,
                hud::hud_system,
                map::map_system,
                menus::menus_system,
            ),
        )
        .add_systems(
            PostUpdate,
            (
                (
                    widgets::bars_system,
                    widgets::cooldowns_system,
                    widgets::key_hints_system,
                    widgets::sliders_system,
                    widgets::toggles_system,
                    toast::toasts_system,
                ),
                layout::ui_layout_system,
                input::log_events_system,
            )
                .chain()
                .in_set(UpdateSet::AfterPropagate),
        )
        .add_render_feature(UiFeature::default());

        api::register(app);
        inventory::register_api(app);
        shop::register_api(app);
        dialogue::register_api(app);
        hud::register_api(app);
        map::register_api(app);
    }
}

fn register_types(app: &mut App) {
    app.register_component::<Node>(
        "A UI element: theme `classes` plus an inline `style` and per-state patches (hover, pressed, focused, selected, disabled). Logical px.",
    )
    .register_component::<Interaction>(
        "UI state: hovered/pressed/focused (set by the UI), selected/disabled (set by the game).",
    )
    .register_component::<Text>(
        "Text in a UI node: markup [b] [i] [color=#hex] [size=N], bindings {path} and {@Entity.Component.field}, typewriter `reveal` (characters).",
    )
    .register_component::<Icon>(
        "An SVG icon or portrait fitted to the node (`ui.icons` lists names), with tint, flip and desaturate.",
    )
    .register_component::<OnClick>(
        "Action string sent as a UiEvent when the node is clicked or accepted, e.g. \"shop.buy\".",
    )
    .register_component::<Focusable>("Reachable with arrow keys, d-pad and left stick.")
    .register_component::<Tooltip>("Tooltip (title, rich text, title color) shown on hover or focus.")
    .register_component::<Draggable>("Can be dragged; `payload` travels to the drop target.")
    .register_component::<DropTarget>(
        "Accepts drops whose payload starts with one of `accepts` (empty = all); `slot` names it.",
    )
    .register_component::<ScrollState>("Scroll offset of an overflow: scroll node (physical px).")
    .register_component::<Screen>(
        "A top-level screen: key, modal (takes keyboard/gamepad and blocks game input), layer, closable.",
    )
    .register_component::<WorldAnchor>(
        "Pins a top-level UI node to a world position or entity (nameplates, markers); can clamp to screen edges and fade with distance (m).",
    )
    .register_component::<CustomPaint>("Drawn by a custom painter by `kind` (maps, charts).")
    .register_component::<UiId>("Stable name of a UI node for events, ui.click and ui.tree.")
    .register_component::<widgets::Bar>(
        "Value bar painted in a bar_track node: value 0..1 or bind/bind_max (data paths), color/color2 gradient, segments, low (pulse threshold), label.",
    )
    .register_component::<widgets::Cooldown>(
        "Cooldown sweep over a slot: remaining/total seconds (or bind/bind_total), label.",
    )
    .register_component::<widgets::KeyHint>(
        "Key prompt showing `key` on keyboard or `pad` (A, B, X, Y, LB, ...) on gamepad.",
    )
    .register_component::<widgets::Slider>(
        "Slider: value, min, max, step, bind (data path or $Resource.field), decimals, suffix.",
    )
    .register_component::<widgets::Toggle>("On/off switch writing a bool to `bind`.")
    .register_component::<input::Adjustable>(
        "Left/right adjusts this node (sends UiEvent::Adjust) instead of moving focus.",
    )
    .register_component::<items::Inventory>(
        "Bag slots [{item, count} or null], equipment {slot: {item, count}}, gold, max_weight (kg).",
    )
    .register_component::<items::Vendor>(
        "A merchant: name, portrait, greeting, stock [{item, count (0 = unlimited)}], markup, buy_rate, gold.",
    )
    .register_component::<toast::Toast>("A toast notification's timing (real seconds).")
    .register_resource::<items::ItemDatabase>(
        "Every item by id: name, icon, rarity 0-4, category, description, value (gold), stack, slot, stats, weight (kg).",
    )
    .register_resource::<dialogue::DialogueLibrary>(
        "Conversations by id: {start, speakers {id: {name, title, portrait, color, side}}, nodes {id: {speaker, text, choices [{text, next, action, requires}], next, action}}}.",
    )
    .register_component::<hud::Compass>(
        "Heading strip (paint: compass): bind (heading path), fov_deg. Shows the waypoint and nearby places.",
    )
    .register_component::<hud::Minimap>(
        "Round minimap (paint: minimap) of the player's planet around them: span_deg, heading (path).",
    )
    .register_component::<hud::Crosshair>("Crosshair (paint: crosshair): spread 0..1 or bind.")
    .register_component::<hud::TimeGauge>("Ring gauge of the engine's time scale (paint: time_gauge).")
    .register_component::<hud::MarkerDistance>("The distance label inside a world marker.")
    .register_component::<hud::MarkerArrow>("The edge arrow inside a world marker.")
    .register_component::<hud::Marker>(
        "World marker: title, subtitle, icon, color, show_distance. Pair with WorldAnchor on a top-level node.",
    )
    .register_resource::<map::MapData>(
        "The universe the map shows: galaxy, systems [{id, name, x, y, star_color, star_class, faction, discovered, planets [{id, name, kind, seed, radius_km, orbit_au, rings, moons, pois}]}], lanes, player {system, planet, lon, lat}, waypoint.",
    )
    .register_resource::<UiSettings>(
        "UI scale (multiplier), reference_height (px the layouts are designed for), min_scale, enabled, debug_outlines, tooltip_delay (real s), block_game_input (while a modal screen is open).",
    )
    .register_resource::<Theme>(
        "Colors (palette) and the style of every class (panel, button, slot, title, ...). Changing it restyles all UI.",
    )
    .register_resource::<UiInputCapture>(
        "Whether the UI owns the pointer / keyboard this frame and whether something is dragged.",
    );
}
