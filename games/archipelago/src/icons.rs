//! The game's own icons. Goods, buildings and ship gear are painted SVGs in `assets/icons`
//! (warm dark outline, three-tone gradients, rim light and a soft contact shadow; regenerate
//! them with `assets/icons/make_icons.py` and `make_gear_icons.py`); the speed, close and
//! arrow glyphs are drawn here.

use engine_ui::icons::IconLibrary;

const OUT: &str =
    r##"stroke="#2a1708" stroke-width="3.5" stroke-linejoin="round" stroke-linecap="round""##;

fn svg(defs: &str, body: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs>{defs}</defs>{body}</svg>"##
    )
}

fn grad(id: &str, a: &str, b: &str) -> String {
    format!(
        r##"<linearGradient id="{id}" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{a}"/><stop offset="1" stop-color="{b}"/></linearGradient>"##
    )
}

/// A compass rose for the sea chart: eight points, the north one red.
fn compass() -> String {
    let mut b = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><circle cx="32" cy="32" r="22" fill="#f3e6c4" fill-opacity="0.55" stroke="#3a2814" stroke-width="1.2"/><circle cx="32" cy="32" r="18.5" fill="none" stroke="#3a2814" stroke-width="0.7" stroke-dasharray="1.6 1.6"/>"##,
    );
    for (deg, long) in [
        (45, false),
        (135, false),
        (225, false),
        (315, false),
        (0, true),
        (90, true),
        (180, true),
        (270, true),
    ] {
        let (tip, side) = if long { (2.0, 5.0) } else { (14.0, 3.6) };
        let dark = if deg == 0 { "#9a2f1c" } else { "#3a2814" };
        b += &format!(
            r##"<g transform="rotate({deg} 32 32)"><path d="M32 {tip} L32 32 L{l} {s} Z" fill="{dark}"/><path d="M32 {tip} L{r} {s} L32 32 Z" fill="#efe0b8" stroke="#3a2814" stroke-width="0.6" stroke-linejoin="round"/></g>"##,
            l = 32.0 - side,
            r = 32.0 + side,
            s = 32.0 - side,
        );
    }
    b += r##"<circle cx="32" cy="32" r="3.2" fill="#c89a3a" stroke="#3a2814" stroke-width="0.8"/></svg>"##;
    b
}

/// The painted goods and buildings.
const PAINTED: &[(&str, &str)] = &[
    ("isle_logs", include_str!("../assets/icons/isle_logs.svg")),
    (
        "isle_planks",
        include_str!("../assets/icons/isle_planks.svg"),
    ),
    ("isle_wool", include_str!("../assets/icons/isle_wool.svg")),
    (
        "isle_clothes",
        include_str!("../assets/icons/isle_clothes.svg"),
    ),
    ("isle_grain", include_str!("../assets/icons/isle_grain.svg")),
    ("isle_flour", include_str!("../assets/icons/isle_flour.svg")),
    ("isle_hops", include_str!("../assets/icons/isle_hops.svg")),
    ("isle_ale", include_str!("../assets/icons/isle_ale.svg")),
    ("isle_clay", include_str!("../assets/icons/isle_clay.svg")),
    (
        "isle_bricks",
        include_str!("../assets/icons/isle_bricks.svg"),
    ),
    ("isle_house", include_str!("../assets/icons/isle_house.svg")),
    (
        "isle_workshop",
        include_str!("../assets/icons/isle_workshop.svg"),
    ),
    ("isle_farm", include_str!("../assets/icons/isle_farm.svg")),
    (
        "isle_chapel",
        include_str!("../assets/icons/isle_chapel.svg"),
    ),
    (
        "isle_anchor",
        include_str!("../assets/icons/isle_anchor.svg"),
    ),
    (
        "isle_airship",
        include_str!("../assets/icons/isle_airship.svg"),
    ),
    ("isle_road", include_str!("../assets/icons/isle_road.svg")),
    (
        "isle_people",
        include_str!("../assets/icons/isle_people.svg"),
    ),
    (
        "isle_demolish",
        include_str!("../assets/icons/isle_demolish.svg"),
    ),
    (
        "isle_market",
        include_str!("../assets/icons/isle_market.svg"),
    ),
    // Goods of the far regions.
    ("isle_cane", include_str!("../assets/icons/isle_cane.svg")),
    ("isle_rum", include_str!("../assets/icons/isle_rum.svg")),
    ("isle_dates", include_str!("../assets/icons/isle_dates.svg")),
    ("isle_salt", include_str!("../assets/icons/isle_salt.svg")),
    ("isle_furs", include_str!("../assets/icons/isle_furs.svg")),
    ("isle_oil", include_str!("../assets/icons/isle_oil.svg")),
    // Goods and buildings of the burghers' tier.
    ("isle_glass", include_str!("../assets/icons/isle_glass.svg")),
    (
        "isle_saltfish",
        include_str!("../assets/icons/isle_saltfish.svg"),
    ),
    (
        "isle_datecakes",
        include_str!("../assets/icons/isle_datecakes.svg"),
    ),
    ("isle_coats", include_str!("../assets/icons/isle_coats.svg")),
    ("isle_lamps", include_str!("../assets/icons/isle_lamps.svg")),
    ("isle_paved", include_str!("../assets/icons/isle_paved.svg")),
    (
        "isle_school",
        include_str!("../assets/icons/isle_school.svg"),
    ),
    (
        "isle_tavern",
        include_str!("../assets/icons/isle_tavern.svg"),
    ),
];

/// Every icon as (name, svg).
/// Every craft, the shipyard and the chandler, painted by `assets/icons/make_craft_icons.py`.
const CRAFT: &[(&str, &str)] = &[
    (
        "isle_craft_schooner",
        include_str!("../assets/icons/isle_craft_schooner.svg"),
    ),
    (
        "isle_craft_barque",
        include_str!("../assets/icons/isle_craft_barque.svg"),
    ),
    (
        "isle_craft_clipper",
        include_str!("../assets/icons/isle_craft_clipper.svg"),
    ),
    (
        "isle_craft_smack",
        include_str!("../assets/icons/isle_craft_smack.svg"),
    ),
    (
        "isle_craft_paddle_steamer",
        include_str!("../assets/icons/isle_craft_paddle_steamer.svg"),
    ),
    (
        "isle_craft_steamer",
        include_str!("../assets/icons/isle_craft_steamer.svg"),
    ),
    (
        "isle_craft_biplane",
        include_str!("../assets/icons/isle_craft_biplane.svg"),
    ),
    (
        "isle_craft_seaplane",
        include_str!("../assets/icons/isle_craft_seaplane.svg"),
    ),
    (
        "isle_shipyard",
        include_str!("../assets/icons/isle_shipyard.svg"),
    ),
    (
        "isle_chandler",
        include_str!("../assets/icons/isle_chandler.svg"),
    ),
    (
        "isle_compass",
        include_str!("../assets/icons/isle_compass.svg"),
    ),
    (
        "isle_craft_cutter",
        include_str!("../assets/icons/isle_craft_cutter.svg"),
    ),
    (
        "isle_craft_brig",
        include_str!("../assets/icons/isle_craft_brig.svg"),
    ),
    (
        "isle_craft_frigate",
        include_str!("../assets/icons/isle_craft_frigate.svg"),
    ),
    (
        "isle_craft_gunboat",
        include_str!("../assets/icons/isle_craft_gunboat.svg"),
    ),
    (
        "isle_craft_liner",
        include_str!("../assets/icons/isle_craft_liner.svg"),
    ),
    (
        "isle_craft_flying_boat",
        include_str!("../assets/icons/isle_craft_flying_boat.svg"),
    ),
    (
        "isle_fire_watch",
        include_str!("../assets/icons/isle_fire_watch.svg"),
    ),
    (
        "isle_watch_house",
        include_str!("../assets/icons/isle_watch_house.svg"),
    ),
    (
        "isle_infirmary",
        include_str!("../assets/icons/isle_infirmary.svg"),
    ),
    ("isle_ruin", include_str!("../assets/icons/isle_ruin.svg")),
    (
        "isle_alert_fire",
        include_str!("../assets/icons/isle_alert_fire.svg"),
    ),
    (
        "isle_alert_riot",
        include_str!("../assets/icons/isle_alert_riot.svg"),
    ),
    (
        "isle_alert_sick",
        include_str!("../assets/icons/isle_alert_sick.svg"),
    ),
];

/// Ship gear and captains, painted by `assets/icons/make_gear_icons.py`.
const GEAR: &[(&str, &str)] = &[
    (
        "isle_gear_armour",
        include_str!("../assets/icons/isle_gear_armour.svg"),
    ),
    (
        "isle_gear_bomb",
        include_str!("../assets/icons/isle_gear_bomb.svg"),
    ),
    (
        "isle_gear_cannon",
        include_str!("../assets/icons/isle_gear_cannon.svg"),
    ),
    (
        "isle_gear_captain",
        include_str!("../assets/icons/isle_gear_captain.svg"),
    ),
    (
        "isle_gear_cat",
        include_str!("../assets/icons/isle_gear_cat.svg"),
    ),
    (
        "isle_gear_chronometer",
        include_str!("../assets/icons/isle_gear_chronometer.svg"),
    ),
    (
        "isle_gear_coal",
        include_str!("../assets/icons/isle_gear_coal.svg"),
    ),
    (
        "isle_gear_cutlass",
        include_str!("../assets/icons/isle_gear_cutlass.svg"),
    ),
    (
        "isle_gear_engine",
        include_str!("../assets/icons/isle_gear_engine.svg"),
    ),
    (
        "isle_gear_flag",
        include_str!("../assets/icons/isle_gear_flag.svg"),
    ),
    (
        "isle_gear_flare",
        include_str!("../assets/icons/isle_gear_flare.svg"),
    ),
    (
        "isle_gear_gas_cell",
        include_str!("../assets/icons/isle_gear_gas_cell.svg"),
    ),
    (
        "isle_gear_hatch",
        include_str!("../assets/icons/isle_gear_hatch.svg"),
    ),
    (
        "isle_gear_horseshoe",
        include_str!("../assets/icons/isle_gear_horseshoe.svg"),
    ),
    (
        "isle_gear_mail",
        include_str!("../assets/icons/isle_gear_mail.svg"),
    ),
    (
        "isle_gear_mine",
        include_str!("../assets/icons/isle_gear_mine.svg"),
    ),
    (
        "isle_gear_overboard",
        include_str!("../assets/icons/isle_gear_overboard.svg"),
    ),
    (
        "isle_gear_propeller",
        include_str!("../assets/icons/isle_gear_propeller.svg"),
    ),
    (
        "isle_gear_purse",
        include_str!("../assets/icons/isle_gear_purse.svg"),
    ),
    (
        "isle_gear_radio",
        include_str!("../assets/icons/isle_gear_radio.svg"),
    ),
    (
        "isle_gear_ram",
        include_str!("../assets/icons/isle_gear_ram.svg"),
    ),
    (
        "isle_gear_repair",
        include_str!("../assets/icons/isle_gear_repair.svg"),
    ),
    (
        "isle_gear_sails",
        include_str!("../assets/icons/isle_gear_sails.svg"),
    ),
    (
        "isle_gear_sheathing",
        include_str!("../assets/icons/isle_gear_sheathing.svg"),
    ),
    (
        "isle_gear_shot",
        include_str!("../assets/icons/isle_gear_shot.svg"),
    ),
    (
        "isle_gear_sights",
        include_str!("../assets/icons/isle_gear_sights.svg"),
    ),
    (
        "isle_gear_smoke",
        include_str!("../assets/icons/isle_gear_smoke.svg"),
    ),
    (
        "isle_gear_spyglass",
        include_str!("../assets/icons/isle_gear_spyglass.svg"),
    ),
    (
        "isle_gear_torpedo",
        include_str!("../assets/icons/isle_gear_torpedo.svg"),
    ),
];

/// The house card's moods, categories, moving up and down and the hold, painted by
/// `assets/icons/make_mood_icons.py`.
const MOOD: &[(&str, &str)] = &[
    (
        "isle_mood_angry",
        include_str!("../assets/icons/isle_mood_angry.svg"),
    ),
    (
        "isle_mood_unhappy",
        include_str!("../assets/icons/isle_mood_unhappy.svg"),
    ),
    (
        "isle_mood_content",
        include_str!("../assets/icons/isle_mood_content.svg"),
    ),
    (
        "isle_mood_happy",
        include_str!("../assets/icons/isle_mood_happy.svg"),
    ),
    (
        "isle_mood_euphoric",
        include_str!("../assets/icons/isle_mood_euphoric.svg"),
    ),
    (
        "isle_cat_food",
        include_str!("../assets/icons/isle_cat_food.svg"),
    ),
    (
        "isle_cat_clothing",
        include_str!("../assets/icons/isle_cat_clothing.svg"),
    ),
    (
        "isle_cat_square",
        include_str!("../assets/icons/isle_cat_square.svg"),
    ),
    (
        "isle_cat_luxury",
        include_str!("../assets/icons/isle_cat_luxury.svg"),
    ),
    (
        "isle_move_up",
        include_str!("../assets/icons/isle_move_up.svg"),
    ),
    (
        "isle_move_down",
        include_str!("../assets/icons/isle_move_down.svg"),
    ),
    ("isle_hold", include_str!("../assets/icons/isle_hold.svg")),
];

/// The relations screen's emblems, treaties and sides, painted by
/// `assets/icons/make_relations_icons.py`.
const RELATIONS: &[(&str, &str)] = &[
    (
        "isle_emblem_gull",
        include_str!("../assets/icons/isle_emblem_gull.svg"),
    ),
    (
        "isle_emblem_lantern",
        include_str!("../assets/icons/isle_emblem_lantern.svg"),
    ),
    (
        "isle_emblem_anvil",
        include_str!("../assets/icons/isle_emblem_anvil.svg"),
    ),
    (
        "isle_treaty_war",
        include_str!("../assets/icons/isle_treaty_war.svg"),
    ),
    (
        "isle_treaty_hostile",
        include_str!("../assets/icons/isle_treaty_hostile.svg"),
    ),
    (
        "isle_treaty_neutral",
        include_str!("../assets/icons/isle_treaty_neutral.svg"),
    ),
    (
        "isle_treaty_pact",
        include_str!("../assets/icons/isle_treaty_pact.svg"),
    ),
    (
        "isle_treaty_alliance",
        include_str!("../assets/icons/isle_treaty_alliance.svg"),
    ),
    (
        "isle_rel_diplomacy",
        include_str!("../assets/icons/isle_rel_diplomacy.svg"),
    ),
    (
        "isle_rel_trade",
        include_str!("../assets/icons/isle_rel_trade.svg"),
    ),
    (
        "isle_rel_military",
        include_str!("../assets/icons/isle_rel_military.svg"),
    ),
    ("isle_gift", include_str!("../assets/icons/isle_gift.svg")),
];

/// The build bar's tier tabs, pad-lock and planning tools, and the pedlar's stall, painted by
/// `assets/icons/make_build_icons.py`.
const TIERS: &[(&str, &str)] = &[
    (
        "isle_tier_fisherfolk",
        include_str!("../assets/icons/isle_tier_fisherfolk.svg"),
    ),
    (
        "isle_tier_labourers",
        include_str!("../assets/icons/isle_tier_labourers.svg"),
    ),
    (
        "isle_tier_burghers",
        include_str!("../assets/icons/isle_tier_burghers.svg"),
    ),
    (
        "isle_pedlar",
        include_str!("../assets/icons/isle_pedlar.svg"),
    ),
    (
        "isle_padlock",
        include_str!("../assets/icons/isle_padlock.svg"),
    ),
    (
        "isle_blueprint",
        include_str!("../assets/icons/isle_blueprint.svg"),
    ),
    (
        "isle_build_all",
        include_str!("../assets/icons/isle_build_all.svg"),
    ),
    ("isle_stamp", include_str!("../assets/icons/isle_stamp.svg")),
    (
        "isle_stamp_copy",
        include_str!("../assets/icons/isle_stamp_copy.svg"),
    ),
];

pub fn all() -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = PAINTED
        .iter()
        .chain(GEAR)
        .chain(CRAFT)
        .chain(MOOD)
        .chain(RELATIONS)
        .chain(TIERS)
        .map(|(n, s)| (*n, s.to_string()))
        .collect();
    v.extend(vec![
        // The speed medallions: dark ink glyphs that read on ivory enamel.
        (
            "isle_speed_pause",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round" stroke-linecap="round"><rect x="17" y="13" width="11" height="38" rx="2.5" fill="url(#sp)"/><rect x="36" y="13" width="11" height="38" rx="2.5" fill="url(#sp)"/></g>"##,
            ),
        ),
        (
            "isle_speed_1",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round" stroke-linecap="round"><path d="M20 11 L50 32 L20 53 Z" fill="url(#sp)"/></g>"##,
            ),
        ),
        (
            "isle_speed_2",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round" stroke-linecap="round"><path d="M8 14 L32 32 L8 50 Z" fill="url(#sp)"/><path d="M32 14 L56 32 L32 50 Z" fill="url(#sp)"/></g>"##,
            ),
        ),
        (
            "isle_speed_4",
            svg(
                &grad("sp", "#8a5a1a", "#3a2408"),
                r##"<g stroke="#2a1708" stroke-width="2.6" stroke-linejoin="round" stroke-linecap="round"><path d="M4 18 L22 32 L4 46 Z" fill="url(#sp)"/><path d="M22 18 L40 32 L22 46 Z" fill="url(#sp)"/><path d="M40 18 L58 32 L40 46 Z" fill="url(#sp)"/></g>"##,
            ),
        ),
        (
            "isle_close",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="11" stroke-linejoin="round" stroke-linecap="round"><path d="M17 17 L47 47 M47 17 L17 47"/></g><g stroke="url(#sp)" stroke-width="5.5" stroke-linecap="round"><path d="M17 17 L47 47 M47 17 L17 47"/></g>"##,
            ),
        ),
        (
            "isle_ledger",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round"><path d="M6 17 Q19 10 31 17 L31 54 Q19 47 6 54 Z" fill="url(#sp)"/><path d="M33 17 Q45 10 58 17 L58 54 Q45 47 33 54 Z" fill="url(#sp)"/></g><g stroke="#f0dfb4" stroke-width="1.8" stroke-linecap="round" fill="none" opacity="0.85"><path d="M11 25 Q19 21 27 25 M11 32 Q19 28 27 32 M11 39 Q19 35 27 39"/><path d="M37 25 Q45 21 53 25 M37 32 Q45 28 53 32 M37 39 Q45 35 53 39"/></g>"##,
            ),
        ),
        (
            "isle_items",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round"><path d="M7 28 L57 28 L57 55 L7 55 Z" fill="url(#sp)"/><path d="M7 28 C7 14 19 9 32 9 C45 9 57 14 57 28 Z" fill="url(#sp)"/></g><g stroke="#f0dfb4" stroke-width="2.2" fill="none" opacity="0.9" stroke-linecap="round"><path d="M18 11.5 L18 55 M46 11.5 L46 55"/><path d="M10 28 L54 28"/></g><g stroke="#2a1708" stroke-width="2"><rect x="27" y="24" width="10" height="13" rx="2" fill="#f0c85a"/></g><circle cx="32" cy="31" r="1.8" fill="#2a1708"/>"##,
            ),
        ),
        (
            "isle_routes",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round" stroke-linecap="round"><circle cx="32" cy="32" r="17" fill="none" stroke-width="9"/><circle cx="32" cy="32" r="17" fill="none" stroke="url(#sp)" stroke-width="4.5"/><path d="M32 4 L32 60 M4 32 L60 32 M12.2 12.2 L51.8 51.8 M51.8 12.2 L12.2 51.8" stroke-width="7"/><path d="M32 4 L32 60 M4 32 L60 32 M12.2 12.2 L51.8 51.8 M51.8 12.2 L12.2 51.8" stroke="url(#sp)" stroke-width="3"/><circle cx="32" cy="32" r="6.5" fill="url(#sp)"/></g>"##,
            ),
        ),
        (
            "isle_relations",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round" stroke-linecap="round"><path d="M14 58 L22 8" stroke-width="7"/><path d="M50 58 L42 8" stroke-width="7"/><path d="M14 58 L22 8 M50 58 L42 8" stroke="url(#sp)" stroke-width="3"/><path d="M22 10 L36 14 L32 22 L40 28 L21 26 Z" fill="url(#sp)"/><path d="M42 10 L28 14 L32 22 L24 28 L43 26 Z" fill="url(#sp)" opacity="0.92"/></g><g stroke="#f0dfb4" stroke-width="1.6" fill="none" opacity="0.8" stroke-linecap="round"><path d="M25 14 L32 16 M39 14 L32 16"/></g>"##,
            ),
        ),
        (
            "isle_plus",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round"><path d="M26 8 L38 8 L38 26 L56 26 L56 38 L38 38 L38 56 L26 56 L26 38 L8 38 L8 26 L26 26 Z" fill="url(#sp)"/></g>"##,
            ),
        ),
        (
            "isle_sort_tier",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round"><path d="M6 56 L6 42 L22 42 L22 56 Z M24 56 L24 28 L40 28 L40 56 Z M42 56 L42 12 L58 12 L58 56 Z" fill="url(#sp)"/></g><g stroke="#f0dfb4" stroke-width="2" fill="none" opacity="0.85" stroke-linecap="round"><path d="M10 46 L18 46 M28 32 L36 32 M46 16 L54 16"/></g>"##,
            ),
        ),
        (
            "isle_sort_type",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round"><rect x="7" y="7" width="22" height="22" rx="3" fill="url(#sp)"/><rect x="35" y="7" width="22" height="22" rx="3" fill="url(#sp)"/><rect x="7" y="35" width="22" height="22" rx="3" fill="url(#sp)"/><rect x="35" y="35" width="22" height="22" rx="3" fill="url(#sp)"/></g><g stroke="#f0dfb4" stroke-width="2" fill="none" opacity="0.85" stroke-linecap="round"><path d="M12 12 L20 12 M40 12 L48 12 M12 40 L20 40 M40 40 L48 40"/></g>"##,
            ),
        ),
        (
            "isle_tradelines",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-linecap="round" fill="none"><path d="M12 48 C18 18 46 18 52 30" stroke-width="7" stroke-dasharray="1 9"/><path d="M12 48 C18 18 46 18 52 30" stroke="url(#sp)" stroke-width="3.6" stroke-dasharray="7 5"/><path d="M14 30 C26 54 44 56 52 50" stroke-width="7" stroke-dasharray="1 9"/><path d="M14 30 C26 54 44 56 52 50" stroke="url(#sp)" stroke-width="3.6" stroke-dasharray="7 5"/></g><g stroke="#2a1708" stroke-width="2.6"><circle cx="12" cy="48" r="6" fill="url(#sp)"/><circle cx="52" cy="30" r="6" fill="url(#sp)"/><circle cx="14" cy="30" r="5" fill="url(#sp)"/><circle cx="52" cy="50" r="5" fill="url(#sp)"/></g>"##,
            ),
        ),
        (
            "isle_left",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round" stroke-linecap="round"><path d="M44 11 L14 32 L44 53 Z" fill="url(#sp)"/></g>"##,
            ),
        ),
        (
            "isle_load",
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><path d="M32 6 L60 54 L4 54 Z" fill="#2f6e28" stroke="#1c3a14" stroke-width="4" stroke-linejoin="round"/></svg>"##.to_string(),
        ),
        (
            "isle_unload",
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><path d="M32 58 L60 10 L4 10 Z" fill="#9a2f1c" stroke="#4a160c" stroke-width="4" stroke-linejoin="round"/></svg>"##.to_string(),
        ),
        (
            "isle_home",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round"><path d="M7 33 L32 10 L57 33 L50 33 L50 55 L14 55 L14 33 Z" fill="url(#sp)"/><path d="M42 18 L42 10 L48 10 L48 24 Z" fill="url(#sp)"/></g><path d="M27 55 L27 42 Q32 37 37 42 L37 55 Z" fill="#f0dfb4" opacity="0.9"/>"##,
            ),
        ),
        (
            "isle_mark_ship",
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><path d="M32 2 C44 16 46 36 43 62 L21 62 C18 36 20 16 32 2 Z" fill="#f6ead0"/><path d="M32 9 C40 20 41 36 39 56 L25 56 C23 36 24 20 32 9 Z" fill="#2a1a0a"/><path d="M32 22 L32 48" stroke="#f6ead0" stroke-width="3"/></svg>"##.to_string(),
        ),
        (
            "isle_mark_airship",
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><ellipse cx="32" cy="30" rx="15" ry="29" fill="#f6ead0"/><path d="M20 50 L12 62 L26 56 Z M44 50 L52 62 L38 56 Z" fill="#f6ead0"/><ellipse cx="32" cy="30" rx="10" ry="24" fill="#2c4a66"/><path d="M23 48 L17 57 L27 53 Z M41 48 L47 57 L37 53 Z" fill="#2c4a66"/><path d="M32 10 L32 50" stroke="#a9c3d6" stroke-width="2.5"/></svg>"##.to_string(),
        ),
        (
            "isle_compass",
            compass(),
        ),
        (
            "isle_arrow",
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 68 40"><path d="M4 20 L50 20" stroke="#6a5339" stroke-width="4" stroke-linecap="round"/><path d="M42 8 L62 20 L42 32 Z" fill="#6a5339"/><circle cx="4" cy="20" r="3.5" fill="#6a5339"/></svg>"##.to_string(),
        ),
        (
            "isle_pointer",
            svg(
                "",
                &format!(r##"<g {OUT}><path d="M14 6 L14 50 L26 40 L34 58 L42 54 L34 37 L50 36 Z" fill="#fffaf0"/></g>"##),
            ),
        ),
    ]);
    v
}

/// Add every icon to the UI's library. An item picture not painted yet shows the anchor.
pub fn install(lib: &mut IconLibrary) {
    let all = all();
    for (name, s) in &all {
        if let Err(e) = lib.add(name, s) {
            engine::core::log::warn!("archipelago icon {name}: {e}");
        }
    }
    if let Some((_, anchor)) = all.iter().find(|(n, _)| *n == "isle_anchor") {
        for d in crate::items::ITEMS.iter() {
            let key = d.icon_key();
            if !lib.contains(key) {
                let _ = lib.add(key, anchor);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_icon_parses() {
        let mut lib = engine_ui::icons::IconLibrary::default();
        for (name, s) in super::all() {
            lib.add(name, &s).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        for g in crate::defs::Good::ALL {
            assert!(lib.contains(g.icon()), "{}", g.icon());
        }
        // Every item has a picture (its own, or the anchor until one is painted).
        let mut lib = engine_ui::icons::IconLibrary::default();
        super::install(&mut lib);
        for d in crate::items::ITEMS.iter() {
            assert!(lib.contains(d.icon_key()), "{}", d.icon_key());
        }
    }
}
