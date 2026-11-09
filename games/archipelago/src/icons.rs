//! The game's own icons. Goods and buildings are painted SVGs in `assets/icons` (warm dark
//! outline, three-tone gradients, rim light and a soft contact shadow; regenerate them with
//! `assets/icons/make_icons.py`); the speed, close and arrow glyphs are drawn here.

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
];

/// Every icon as (name, svg).
pub fn all() -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> =
        PAINTED.iter().map(|(n, s)| (*n, s.to_string())).collect();
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

/// Add every icon to the UI's library.
pub fn install(lib: &mut IconLibrary) {
    for (name, s) in all() {
        if let Err(e) = lib.add(name, &s) {
            engine::core::log::warn!("archipelago icon {name}: {e}");
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
    }
}
