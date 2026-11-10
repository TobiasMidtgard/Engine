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
            "isle_ledger",
            svg(
                &grad("sp", "#5a4128", "#24170b"),
                r##"<g stroke="#2a1708" stroke-width="3" stroke-linejoin="round"><path d="M6 17 Q19 10 31 17 L31 54 Q19 47 6 54 Z" fill="url(#sp)"/><path d="M33 17 Q45 10 58 17 L58 54 Q45 47 33 54 Z" fill="url(#sp)"/></g><g stroke="#f0dfb4" stroke-width="1.8" stroke-linecap="round" fill="none" opacity="0.85"><path d="M11 25 Q19 21 27 25 M11 32 Q19 28 27 32 M11 39 Q19 35 27 39"/><path d="M37 25 Q45 21 53 25 M37 32 Q45 28 53 32 M37 39 Q45 35 53 39"/></g>"##,
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
