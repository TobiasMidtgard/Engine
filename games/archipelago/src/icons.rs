//! The game's own icons: goods, building groups and the top bar, drawn in the UI kit's style
//! (bold dark outline, a soft gradient fill and one highlight stroke) so they sit beside its
//! built-in fish, bread and coins.

use engine_ui::icons::IconLibrary;

const OUT: &str =
    r##"stroke="#0c0f16" stroke-width="3.5" stroke-linejoin="round" stroke-linecap="round""##;

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

/// Every icon as (name, svg).
pub fn all() -> Vec<(&'static str, String)> {
    vec![
        (
            "isle_logs",
            svg(
                &(grad("bark", "#a8743f", "#5a3519") + &grad("cut", "#f3d49a", "#c79a5a")),
                &format!(
                    r##"<g {OUT}><path d="M10 40 L44 22 L52 34 L18 52 Z" fill="url(#bark)"/><ellipse cx="14" cy="46" rx="6" ry="7" fill="url(#cut)"/>
                    <path d="M14 30 L44 14 L50 24 L20 40 Z" fill="url(#bark)"/><ellipse cx="17" cy="35" rx="5" ry="6" fill="url(#cut)"/></g>
                    <g fill="none" stroke="#7a4a22" stroke-width="1.2"><circle cx="14" cy="46" r="2.5"/><circle cx="17" cy="35" r="2"/></g>
                    <path d="M24 44 L46 32 M22 33 L42 22" stroke="#d9a56b" stroke-width="1.5" opacity=".8"/>"##
                ),
            ),
        ),
        (
            "isle_planks",
            svg(
                &grad("pl", "#f0c98a", "#a8723c"),
                &format!(
                    r##"<g {OUT} fill="url(#pl)"><path d="M6 44 L40 54 L58 44 L24 34 Z"/><path d="M6 36 L40 46 L58 36 L24 26 Z"/><path d="M6 28 L40 38 L58 28 L24 18 Z"/></g>
                    <path d="M12 29 L38 36 M14 37 L36 43" stroke="#8a5a2c" stroke-width="1.3" opacity=".7"/>"##
                ),
            ),
        ),
        (
            "isle_wool",
            svg(
                &grad("wl", "#ffffff", "#c9c2b0"),
                &format!(
                    r##"<g {OUT} fill="url(#wl)"><path d="M14 44 C6 44 6 32 14 31 C13 22 24 18 29 24 C33 15 47 17 47 27 C56 26 59 38 50 42 C50 50 40 52 36 47 C31 53 18 52 14 44 Z"/></g>
                    <g fill="none" stroke="#a89f88" stroke-width="1.5"><path d="M20 36 C22 33 26 33 27 36"/><path d="M33 31 C35 28 39 28 40 31"/><path d="M36 41 C38 38 42 38 43 41"/></g>"##
                ),
            ),
        ),
        (
            "isle_clothes",
            svg(
                &grad("cl", "#5d8fd0", "#2c4f86"),
                &format!(
                    r##"<g {OUT}><path d="M22 10 L32 15 L42 10 L56 18 L50 30 L44 27 L44 54 L20 54 L20 27 L14 30 L8 18 Z" fill="url(#cl)"/></g>
                    <path d="M26 12 C28 18 36 18 38 12" fill="none" stroke="#0c0f16" stroke-width="2"/>
                    <path d="M20 38 L44 38" stroke="#e8c45a" stroke-width="3"/><path d="M15 21 L11 18" stroke="#9cc0f0" stroke-width="1.6"/>"##
                ),
            ),
        ),
        (
            "isle_grain",
            svg(
                &grad("gr", "#ffe08a", "#c48a2a"),
                &format!(
                    r##"<g {OUT} fill="url(#gr)"><path d="M30 58 L30 30" fill="none"/><path d="M30 58 L20 36" fill="none"/><path d="M30 58 L42 34" fill="none"/>
                    <ellipse cx="30" cy="22" rx="5" ry="12"/><ellipse cx="18" cy="28" rx="4.5" ry="11" transform="rotate(-24 18 28)"/><ellipse cx="44" cy="26" rx="4.5" ry="11" transform="rotate(26 44 26)"/></g>
                    <path d="M30 14 L30 30 M18 20 L20 36 M44 18 L42 34" stroke="#a06a1a" stroke-width="1.2"/>"##
                ),
            ),
        ),
        (
            "isle_flour",
            svg(
                &grad("sk", "#f4ead2", "#bfae86"),
                &format!(
                    r##"<g {OUT}><path d="M16 22 C14 30 12 44 16 54 L48 54 C52 44 50 30 48 22 Z" fill="url(#sk)"/><path d="M18 22 C22 14 42 14 46 22 C40 26 24 26 18 22 Z" fill="#ffffff"/></g>
                    <path d="M32 9 L32 16 M27 11 L37 11" stroke="#0c0f16" stroke-width="2.5"/><circle cx="32" cy="40" r="6" fill="none" stroke="#9c8a60" stroke-width="2"/>"##
                ),
            ),
        ),
        (
            "isle_hops",
            svg(
                &grad("hp", "#b8e070", "#4c8a2c"),
                &format!(
                    r##"<path d="M32 4 C30 10 34 12 32 16" fill="none" stroke="#0c0f16" stroke-width="3"/>
                    <g {OUT}><path d="M32 14 C20 18 16 30 20 42 C24 54 32 60 32 60 C32 60 40 54 44 42 C48 30 44 18 32 14 Z" fill="url(#hp)"/></g>
                    <g fill="none" stroke="#2f5e1a" stroke-width="1.8"><path d="M22 28 C28 32 36 32 42 28"/><path d="M21 38 C28 43 36 43 43 38"/><path d="M25 48 C29 51 35 51 39 48"/></g>"##
                ),
            ),
        ),
        (
            "isle_ale",
            svg(
                &(grad("al", "#ffc94a", "#c47414") + &grad("gl", "#e6eef2", "#a9b8c0")),
                &format!(
                    r##"<g {OUT}><path d="M16 18 L44 18 L42 56 L18 56 Z" fill="url(#al)"/><path d="M44 26 C54 26 54 44 43 44" fill="none"/><path d="M14 18 C12 8 24 8 26 12 C30 6 40 8 40 12 C46 10 48 18 44 20 L16 20 Z" fill="#fffaf0"/></g>
                    <path d="M22 26 L21 50" stroke="#ffe7a0" stroke-width="2.5" opacity=".8"/>"##
                ),
            ),
        ),
        (
            "isle_clay",
            svg(
                &grad("cy", "#d08a5a", "#7a3a1e"),
                &format!(
                    r##"<g {OUT} fill="url(#cy)"><path d="M8 46 C10 34 22 28 32 28 C44 28 56 36 56 46 C56 52 48 54 32 54 C16 54 8 52 8 46 Z"/></g>
                    <path d="M16 40 C22 34 30 33 36 34" fill="none" stroke="#e8b08a" stroke-width="2"/><path d="M38 44 C42 42 46 43 48 46" fill="none" stroke="#5a2410" stroke-width="1.6"/>"##
                ),
            ),
        ),
        (
            "isle_bricks",
            svg(
                &grad("bk", "#d46a44", "#8a2e18"),
                &format!(
                    r##"<g {OUT} fill="url(#bk)"><rect x="6" y="38" width="25" height="13" rx="1.5"/><rect x="33" y="38" width="25" height="13" rx="1.5"/><rect x="19" y="23" width="25" height="13" rx="1.5"/></g>
                    <path d="M10 42 L26 42 M37 42 L53 42 M23 27 L39 27" stroke="#f29a72" stroke-width="1.6"/>"##
                ),
            ),
        ),
        (
            "isle_house",
            svg(
                &(grad("wa", "#f4e6c8", "#c8b08a") + &grad("rf", "#d0583a", "#842a18")),
                &format!(
                    r##"<g {OUT}><path d="M12 30 L12 56 L52 56 L52 30" fill="url(#wa)"/><path d="M6 32 L32 10 L58 32 Z" fill="url(#rf)"/><rect x="27" y="40" width="10" height="16" fill="#6a4426"/><rect x="16" y="36" width="7" height="7" fill="#8fc8e8"/><rect x="41" y="36" width="7" height="7" fill="#8fc8e8"/></g>"##
                ),
            ),
        ),
        (
            "isle_workshop",
            svg(
                &(grad("ws", "#c9b08a", "#7a6040") + &grad("sl", "#7a8a9a", "#3a4654")),
                &format!(
                    r##"<g {OUT}><rect x="40" y="10" width="8" height="18" fill="#9a5038"/><path d="M8 28 L8 56 L56 56 L56 28" fill="url(#ws)"/><path d="M4 30 L22 16 L40 30 L58 30 L58 34 L4 34 Z" fill="url(#sl)"/><rect x="16" y="40" width="14" height="16" fill="#5a3a22"/><rect x="38" y="40" width="10" height="8" fill="#ffd27a"/></g>"##
                ),
            ),
        ),
        (
            "isle_farm",
            svg(
                &(grad("fd", "#f2d36a", "#b8861e") + &grad("bn", "#c0472e", "#6e2014")),
                &format!(
                    r##"<g {OUT}><path d="M2 52 L62 52 L62 60 L2 60 Z" fill="url(#fd)"/><path d="M10 52 L10 30 L24 18 L38 30 L38 52 Z" fill="url(#bn)"/><path d="M18 52 L18 40 L30 40 L30 52" fill="#5a2010"/></g>
                    <g stroke="#0c0f16" stroke-width="2.5" fill="none"><path d="M46 52 L46 34 M42 38 L46 34 L50 38 M54 52 L54 40"/></g><path d="M18 40 L30 52 M30 40 L18 52" stroke="#f0d0b0" stroke-width="1.5"/>"##
                ),
            ),
        ),
        (
            "isle_chapel",
            svg(
                &(grad("st", "#e8e2d2", "#a8a090") + &grad("sr", "#5a6878", "#2a3440")),
                &format!(
                    r##"<g {OUT}><path d="M24 30 L24 58 L40 58 L40 30" fill="url(#st)"/><path d="M22 32 L32 8 L42 32 Z" fill="url(#sr)"/><path d="M8 40 L8 58 L24 58 L24 40 L16 32 Z" fill="url(#st)"/><path d="M40 40 L40 58 L56 58 L56 40 L48 32 Z" fill="url(#st)"/><path d="M29 58 L29 46 C29 42 35 42 35 46 L35 58" fill="#4a3020"/></g>
                    <path d="M32 2 L32 10 M28.5 5 L35.5 5" stroke="#e8c45a" stroke-width="2.4"/>"##
                ),
            ),
        ),
        (
            "isle_anchor",
            svg(
                &grad("an", "#9ab8d0", "#3c5670"),
                &format!(
                    r##"<g {OUT} fill="none"><circle cx="32" cy="12" r="5" fill="url(#an)"/><path d="M32 17 L32 56"/><path d="M22 26 L42 26"/><path d="M10 38 C10 50 22 56 32 56 C42 56 54 50 54 38"/><path d="M6 42 L10 36 L15 41 M58 42 L54 36 L49 41"/></g>
                    <g stroke="#9ab8d0" stroke-width="1.6" fill="none"><path d="M32 20 L32 52"/><path d="M13 40 C14 48 22 53 31 53"/></g>"##
                ),
            ),
        ),
        (
            "isle_airship",
            svg(
                &grad("en", "#f0e6cc", "#a8987a"),
                &format!(
                    r##"<g {OUT}><path d="M4 26 C4 14 26 10 40 12 C52 14 58 20 58 26 C58 32 52 38 40 40 C26 42 4 38 4 26 Z" fill="url(#en)"/><path d="M50 14 L60 6 L60 22 Z M50 38 L60 46 L60 30 Z" fill="#c0472e"/><rect x="20" y="44" width="18" height="8" rx="2" fill="#7a5030"/></g>
                    <path d="M24 40 L22 44 M34 40 L36 44" stroke="#0c0f16" stroke-width="2"/><path d="M12 22 C20 16 34 15 44 17" fill="none" stroke="#fffaf0" stroke-width="2" opacity=".9"/>"##
                ),
            ),
        ),
        (
            "isle_road",
            svg(
                &grad("rd", "#c8b48a", "#8a7450"),
                &format!(
                    r##"<g {OUT}><path d="M22 60 L28 4 L36 4 L42 60 Z" fill="url(#rd)"/></g><path d="M32 10 L32 18 M32 26 L32 34 M32 42 L32 52" stroke="#f4ead2" stroke-width="2.5"/>
                    <g {OUT} fill="#5a9a3a"><circle cx="12" cy="20" r="6"/><circle cx="52" cy="40" r="6"/></g>"##
                ),
            ),
        ),
        (
            "isle_people",
            svg(
                &(grad("pa", "#f0c8a0", "#c08a60") + &grad("pb", "#5d8fd0", "#2c4f86")),
                &format!(
                    r##"<g {OUT}><circle cx="22" cy="20" r="8" fill="url(#pa)"/><path d="M8 54 C8 38 14 32 22 32 C30 32 36 38 36 54 Z" fill="url(#pb)"/><circle cx="42" cy="22" r="7" fill="url(#pa)"/><path d="M32 54 C32 40 36 34 42 34 C50 34 56 40 56 54 Z" fill="#c0472e"/></g>"##
                ),
            ),
        ),
        (
            "isle_demolish",
            svg(
                &grad("dm", "#c0c8d0", "#6a7480"),
                &format!(
                    r##"<g {OUT}><path d="M30 34 L10 54 L4 48 L24 28 Z" fill="#9a6a3a"/><path d="M20 22 L36 6 C44 8 52 14 58 24 L50 30 C46 24 42 20 36 18 L28 28 Z" fill="url(#dm)"/></g>"##
                ),
            ),
        ),
        (
            "isle_pointer",
            svg(
                "",
                &format!(
                    r##"<g {OUT}><path d="M14 6 L14 50 L26 40 L34 58 L42 54 L34 37 L50 36 Z" fill="#fffaf0"/></g>"##
                ),
            ),
        ),
    ]
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
