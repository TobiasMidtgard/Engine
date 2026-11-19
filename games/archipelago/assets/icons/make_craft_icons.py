# Painted-style icons for every craft, the shipyard and the ship chandler (isle_craft_*,
# isle_shipyard, isle_chandler, isle_compass), the watch, infirmary and ruin, and the fire,
# riot and sickness alerts, in the same hand as make_icons.py.
# Usage: python3 make_craft_icons.py <out dir>
import os, sys
O = 'stroke="#2a1708" stroke-width="2.4" stroke-linejoin="round" stroke-linecap="round"'
def o(w):
    return f'stroke="#2a1708" stroke-width="{w}" stroke-linejoin="round" stroke-linecap="round"'
def lg(id, *stops, x1=0, y1=0, x2=0, y2=1):
    st = ''.join(f'<stop offset="{p}" stop-color="{c}"/>' for p, c in stops)
    return f'<linearGradient id="{id}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">{st}</linearGradient>'
SH = '<radialGradient id="sh" cx=".5" cy=".5" r=".5"><stop offset="0" stop-color="#000" stop-opacity=".45"/><stop offset="1" stop-color="#000" stop-opacity="0"/></radialGradient>'
def shadow(cx=32, cy=58, rx=26, ry=3.5):
    return f'<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="url(#sh)"/>'
def svg(defs, body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs>{SH}{defs}</defs>{body}</svg>'
HULL = lg('hl', (0, '#4a3a2a'), (.5, '#2a1e14'), (1, '#120c06'))
HULL_W = lg('hw', (0, '#e8e0d0'), (1, '#a8a090'))
RED_LEAD = lg('rl', (0, '#c0503a'), (1, '#6a2414'))
WOOD = lg('wd', (0, '#d8a46a'), (.55, '#9a6432'), (1, '#5a3414'))
CANVAS = lg('cv', (0, '#fffaf0'), (.6, '#eadcb8'), (1, '#b8a47a'), x2=1, y2=0)
SEA = lg('sea', (0, '#6ab8e8'), (1, '#1a4a78'))
FUNNEL = lg('fn', (0, '#e0583a'), (.5, '#b03a24'), (1, '#6a1a0e'), x2=1, y2=0)
IRON = lg('ir', (0, '#b8c0c8'), (.5, '#6a737c'), (1, '#2c3238'))
DEFS = HULL + HULL_W + RED_LEAD + WOOD + CANVAS + SEA + FUNNEL + IRON

def sea():
    return (f'<g {O}><path d="M2 52 C10 49 16 55 24 52 C32 49 38 55 46 52 C52 50 58 53 62 51 L62 62 L2 62 Z" fill="url(#sea)"/></g>'
            '<path d="M6 55 C12 53 16 57 22 55 M38 55 C44 53 48 57 54 55" stroke="#d8f0ff" stroke-width="1.3" fill="none" opacity=".8"/>')

def hull(x0, x1, top, wl, bottom, fill='url(#hl)', bow_rake=6, stern=3):
    # Bow on the right.
    return (f'<g {O}><path d="M{x0} {top} L{x1} {top - 2} L{x1 - bow_rake} {bottom} L{x0 + stern} {bottom} Z" fill="{fill}"/></g>'
            f'<path d="M{x0 + stern * .6} {wl} L{x1 - bow_rake * .45} {wl}" stroke="#c0503a" stroke-width="2.4"/>'
            f'<path d="M{x0 + 2} {top + 1.6} L{x1 - 3} {top - .4}" stroke="#d8c8a8" stroke-width="1.2" opacity=".8"/>')

def mast(x, top, foot, w=1.8):
    return f'<path d="M{x} {foot} L{x} {top}" {o(w + 1.6)}/><path d="M{x} {foot} L{x} {top}" stroke="#b08050" stroke-width="{w * .6}"/>'

def square_sails(x, top, foot, widths):
    out, y = '', top
    h = (foot - top) / len(widths)
    for w in widths:
        out += (f'<g {o(1.8)}><path d="M{x - w} {y + 1} L{x + w} {y + 1} C{x + w + 2} {y + h * .5} {x + w + 1} {y + h - 1} {x + w - 1} {y + h - 1} '
                f'L{x - w + 1} {y + h - 1} C{x - w - 1} {y + h - 1} {x - w - 2} {y + h * .5} {x - w} {y + 1} Z" fill="url(#cv)"/></g>')
        y += h
    return out

I = {}
I['isle_craft_schooner'] = svg(DEFS,
    sea() + mast(22, 8, 44) + mast(38, 6, 44) +
    f'<g {o(1.8)}><path d="M23 10 L23 42 L36 42 C34 30 30 18 23 10 Z" fill="url(#cv)"/>'
    '<path d="M39 8 L39 42 L54 42 C52 28 46 16 39 8 Z" fill="url(#cv)"/>'
    '<path d="M38 12 L58 40 L52 41 Z" fill="url(#cv)"/></g>' +
    hull(8, 58, 44, 49, 53) +
    '<path d="M38 6 L46 4 L38 9" fill="#c0402a" stroke="#2a1708" stroke-width="1.2"/>')
I['isle_craft_barque'] = svg(DEFS,
    sea() + mast(18, 10, 42) + mast(32, 6, 42) + mast(46, 10, 42) +
    square_sails(18, 12, 40, (7, 8, 9)) + square_sails(32, 8, 40, (7, 8, 9, 10)) +
    f'<g {o(1.8)}><path d="M47 12 L47 40 L56 40 C55 30 52 20 47 12 Z" fill="url(#cv)"/></g>' +
    hull(5, 60, 42, 48, 53))
I['isle_craft_clipper'] = svg(DEFS,
    sea() + mast(16, 6, 44) + mast(30, 3, 44) + mast(44, 6, 44) +
    square_sails(16, 7, 42, (6, 7, 8, 8)) + square_sails(30, 4, 42, (6, 7, 8, 9, 9)) + square_sails(44, 7, 42, (6, 7, 8, 8)) +
    f'<g {o(1.6)}><path d="M44 10 L62 42 L52 42 Z" fill="url(#cv)"/></g>' +
    hull(4, 62, 44, 49, 53, bow_rake=9))
I['isle_craft_smack'] = svg(DEFS + lg('rs', (0, '#e8804a'), (.6, '#b0442a'), (1, '#6a2414'), x2=1, y2=0),
    sea() + mast(30, 10, 44) +
    f'<g {o(1.8)}><path d="M31 12 L31 42 L48 42 C46 30 40 18 31 12 Z" fill="url(#rs)"/>'
    '<path d="M29 16 L14 42 L29 42 Z" fill="url(#rs)"/></g>'
    '<path d="M42 40 L56 30" stroke="#2a1708" stroke-width="1.4"/>' +
    hull(12, 54, 44, 49, 53, fill='url(#wd)') +
    '<g fill="#3a7a9a" stroke="#2a1708" stroke-width="1.2"><path d="M50 30 C54 28 58 30 58 32 C56 34 52 34 50 30 Z"/></g>')
I['isle_craft_paddle_steamer'] = svg(DEFS,
    sea() +
    f'<g {O}><rect x="36" y="12" width="7" height="22" fill="url(#fn)"/><path d="M35 12 L44 12" stroke-width="3"/></g>'
    '<g fill="#d8d8d0" opacity=".85" stroke="#2a1708" stroke-width="1.2"><path d="M34 10 C30 10 30 4 35 5 C36 1 43 1 43 6 C47 6 47 10 43 10 Z"/></g>'
    f'<g {O}><rect x="14" y="30" width="38" height="10" fill="url(#hw)"/></g>'
    '<g fill="#3a5a8a">' + ''.join(f'<rect x="{x}" y="33" width="3.4" height="3.4"/>' for x in (18, 24, 30, 46)) + '</g>' +
    hull(6, 60, 40, 47, 52, fill='url(#hl)') +
    f'<g {O}><path d="M22 40 C22 30 38 30 38 40 Z" fill="url(#wd)"/></g>'
    '<g stroke="#2a1708" stroke-width="1.4"><path d="M30 40 L30 32 M30 40 L24.5 34.5 M30 40 L35.5 34.5"/></g>')
I['isle_craft_steamer'] = svg(DEFS,
    sea() +
    f'<g {O}><rect x="30" y="14" width="8" height="18" fill="url(#fn)"/><path d="M29 14 L39 14" stroke-width="3"/></g>'
    '<path d="M30 20 L38 20" stroke="#2a1708" stroke-width="2.4"/>'
    '<g fill="#5a5a58" opacity=".7"><circle cx="30" cy="9" r="4"/><circle cx="24" cy="6" r="3"/><circle cx="18" cy="5" r="2.2"/></g>'
    f'<g {O}><rect x="24" y="30" width="20" height="10" fill="url(#hw)"/><rect x="40" y="24" width="8" height="8" fill="url(#hw)"/></g>' +
    mast(12, 16, 40, 1.4) + mast(54, 18, 40, 1.4) +
    '<g fill="#3a5a8a">' + ''.join(f'<rect x="{x}" y="33" width="3" height="3"/>' for x in (27, 32, 37)) + '</g>' +
    hull(4, 62, 40, 47, 53, bow_rake=7))
I['isle_craft_biplane'] = svg(DEFS + lg('wg', (0, '#f0d890'), (.6, '#c8a040'), (1, '#7a5a14')),
    shadow(32, 60, 22, 3) +
    f'<g {O}><path d="M6 18 L58 18 L56 24 L8 24 Z" fill="url(#wg)"/><path d="M8 38 L56 38 L54 44 L10 44 Z" fill="url(#wg)"/></g>'
    '<g stroke="#2a1708" stroke-width="1.8"><path d="M16 24 L16 38 M48 24 L48 38 M16 24 L48 38 M48 24 L16 38"/></g>'
    f'<g {O}><path d="M26 22 C26 18 38 18 38 22 L38 46 C38 50 26 50 26 46 Z" fill="url(#fn)"/>'
    '<circle cx="32" cy="22" r="4" fill="url(#ir)"/></g>'
    '<path d="M18 14 C26 12 38 12 46 14 M18 30 C26 28 38 28 46 30" stroke="#2a1708" stroke-width="3" stroke-linecap="round" opacity="0"/>'
    '<g stroke="#2a1708" stroke-width="2.2" stroke-linecap="round"><path d="M20 14 L44 14"/></g>'
    '<path d="M20 14 L44 14" stroke="#e0e0e0" stroke-width="1" opacity=".8"/>'
    '<path d="M28 48 L36 48 L34 56 L30 56 Z" fill="url(#wg)" stroke="#2a1708" stroke-width="1.6"/>'
    '<path d="M10 19 L56 19" stroke="#fff4c0" stroke-width="1.2" opacity=".85"/>')
I['isle_craft_seaplane'] = svg(DEFS + lg('pl', (0, '#e8eef4'), (.6, '#a8b4c0'), (1, '#5a6470')),
    sea() +
    f'<g {O}><path d="M8 26 C8 22 18 20 34 20 C48 20 56 22 58 26 C56 30 48 32 34 32 C18 32 8 30 8 26 Z" fill="url(#pl)"/>'
    '<path d="M24 26 L46 26 L44 30 L26 30 Z" fill="url(#pl)"/>'
    '<path d="M6 14 L12 14 L16 24 L10 24 Z" fill="url(#fn)"/>'
    '<path d="M18 22 L50 22" stroke-width="3"/>'
    '<path d="M14 44 L34 44 C36 44 38 46 36 48 L16 48 C14 48 12 46 14 44 Z" fill="url(#pl)"/>'
    '<path d="M36 44 L56 44 C58 44 60 46 58 48 L38 48 C36 48 34 46 36 44 Z" fill="url(#pl)"/></g>'
    '<g stroke="#2a1708" stroke-width="1.8"><path d="M22 32 L22 44 M46 32 L46 44 M28 32 L24 44 M42 32 L44 44"/></g>'
    '<path d="M58 20 L58 32" stroke="#2a1708" stroke-width="2.6" stroke-linecap="round"/>'
    '<g fill="#3a5a8a" stroke="#2a1708" stroke-width="1"><path d="M38 22 L46 22 L48 25 L38 25 Z"/></g>'
    '<path d="M14 24 C22 22 34 21 48 22" stroke="#fff" stroke-width="1.4" fill="none" opacity=".85"/>')
I['isle_shipyard'] = svg(DEFS,
    shadow(32, 60, 28, 3) +
    f'<g {O}><path d="M2 54 L62 46 L62 52 L2 60 Z" fill="url(#wd)"/></g>'
    '<g stroke="#2a1708" stroke-width="1.2" opacity=".7"><path d="M12 53 L12 58 M26 51 L26 56 M40 49 L40 54 M54 47 L54 52"/></g>'
    f'<g {o(3.4)} fill="none"><path d="M12 48 C10 38 12 30 16 26 M22 47 C20 36 22 26 26 22 M32 46 C30 34 32 24 36 20 M42 45 C40 34 42 26 46 22 M52 44 C50 36 52 30 56 28"/></g>'
    '<g stroke="#e0b070" stroke-width="1.4" fill="none"><path d="M12 48 C10 38 12 30 16 26 M22 47 C20 36 22 26 26 22 M32 46 C30 34 32 24 36 20 M42 45 C40 34 42 26 46 22 M52 44 C50 36 52 30 56 28"/></g>'
    f'<g {O}><path d="M8 48 L58 42 L58 46 L8 52 Z" fill="url(#wd)"/></g>'
    '<path d="M14 24 L58 26" stroke="#2a1708" stroke-width="3"/><path d="M14 24 L58 26" stroke="#b08050" stroke-width="1.2"/>'
    f'<g {O}><path d="M6 50 L6 6 L10 6 L10 50 Z" fill="url(#wd)"/><path d="M6 8 L34 4 L34 8 L10 11 Z" fill="url(#wd)"/></g>'
    '<path d="M30 7 L30 16" stroke="#2a1708" stroke-width="1.4"/>'
    f'<g {o(1.6)}><path d="M27 16 L33 16 L32 20 L28 20 Z" fill="url(#ir)"/></g>')
I['isle_chandler'] = svg(DEFS + lg('rf', (0, '#4a6a8a'), (1, '#1a2a3a')) + lg('rp', (0, '#f0d090'), (1, '#a07a3a')),
    shadow(32, 60, 28, 3) +
    f'<g {O}><path d="M6 28 L6 58 L50 58 L50 28 Z" fill="url(#wd)"/><path d="M2 30 L28 14 L54 30 Z" fill="url(#rf)"/></g>'
    f'<g {o(1.8)}><rect x="12" y="38" width="14" height="10" fill="#ffd27a"/><path d="M32 58 L32 40 L44 40 L44 58 Z" fill="#4a2a14"/></g>'
    '<path d="M19 38 L19 48" stroke="#2a1708" stroke-width="1.1"/>'
    '<path d="M50 32 L60 32" stroke="#2a1708" stroke-width="3.4"/><path d="M58 32 L58 36" stroke="#2a1708" stroke-width="1.4"/>'
    f'<g {o(2)}><circle cx="58" cy="44" r="7" fill="url(#rp)"/><circle cx="58" cy="44" r="3.6" fill="none"/></g>'
    '<path d="M53 41 C55 39 59 39 61 41" stroke="#fff2c8" stroke-width="1.2" fill="none"/>'
    '<g stroke="#2a1708" stroke-width="2.6" fill="none" stroke-linecap="round"><path d="M28 18 L28 30 M24 21 L32 21 M23 27 C25 31 31 31 33 27"/></g>'
    '<g stroke="#f0c85a" stroke-width="1.2" fill="none" stroke-linecap="round"><path d="M28 18 L28 30 M24 21 L32 21 M23 27 C25 31 31 31 33 27"/></g>'
    '<path d="M6 31 L26 18" stroke="#8ab0d8" stroke-width="1.6" opacity=".8"/>')

# The second fleet: a revenue cutter, a brig, a frigate, an iron gunboat, a liner and a
# flying boat.
I['isle_craft_cutter'] = svg(DEFS,
    sea() + mast(28, 6, 44) +
    f'<g {o(1.8)}><path d="M29 8 L29 42 L48 42 C46 30 38 16 29 8 Z" fill="url(#cv)"/>'
    '<path d="M27 10 L10 42 L27 42 Z" fill="url(#cv)"/>'
    '<path d="M29 9 L40 7 L46 42" fill="none"/></g>' +
    hull(10, 56, 44, 49, 53, fill='url(#hw)', bow_rake=7) +
    '<path d="M13 46 L53 45" stroke="#2a4a7a" stroke-width="2.2"/>'
    '<path d="M28 6 L36 4 L28 9" fill="#2a4a7a" stroke="#2a1708" stroke-width="1.2"/>')
I['isle_craft_brig'] = svg(DEFS,
    sea() + mast(22, 6, 42) + mast(40, 8, 42) +
    square_sails(22, 8, 40, (7, 8, 9, 10)) + square_sails(40, 10, 40, (6, 7, 8, 9)) +
    f'<g {o(1.6)}><path d="M42 12 L60 40 L50 40 Z" fill="url(#cv)"/></g>' +
    hull(6, 60, 42, 48, 53, fill='url(#wd)'))
I['isle_craft_frigate'] = svg(DEFS,
    sea() + mast(16, 5, 40) + mast(31, 2, 40) + mast(46, 6, 40) +
    square_sails(16, 6, 36, (6, 7, 8)) + square_sails(31, 3, 36, (6, 7, 8, 9)) + square_sails(46, 7, 36, (5, 6, 7)) +
    f'<g {o(1.6)}><path d="M46 10 L62 38 L54 38 Z" fill="url(#cv)"/></g>'
    f'<g {O}><path d="M3 38 L61 36 L55 54 L7 54 Z" fill="url(#hl)"/></g>'
    '<path d="M5 43 L59 41.5" stroke="#e8c860" stroke-width="3.6"/>'
    '<g fill="#120c06">' + ''.join(f'<rect x="{x}" y="41.2" width="3" height="2.6"/>' for x in range(10, 56, 6)) + '</g>'
    '<path d="M7 49 L57 48" stroke="#c0503a" stroke-width="2.2"/>'
    '<path d="M31 2 L40 0.5 L31 5" fill="#e8e0d0" stroke="#2a1708" stroke-width="1.2"/>')
I['isle_craft_gunboat'] = svg(DEFS,
    sea() +
    f'<g {O}><rect x="28" y="20" width="7" height="16" fill="url(#ir)"/><path d="M27 20 L36 20" stroke-width="3"/></g>'
    '<g fill="#5a5a58" opacity=".7"><circle cx="28" cy="15" r="3.6"/><circle cx="22" cy="12" r="2.6"/></g>'
    f'<g {O}><rect x="18" y="34" width="22" height="8" fill="url(#ir)"/>'
    '<path d="M42 38 C42 32 52 32 52 38 Z" fill="url(#ir)"/></g>'
    f'<path d="M50 34 L61 31" {o(4.4)}/><path d="M50 34 L61 31" stroke="#8a939c" stroke-width="2"/>' +
    mast(14, 18, 40, 1.4) +
    hull(4, 62, 42, 47, 52, fill='url(#ir)', bow_rake=5) +
    '<path d="M8 44 L58 43" stroke="#2a3238" stroke-width="1.4" opacity=".7"/>')
I['isle_craft_liner'] = svg(DEFS,
    sea() +
    ''.join(f'<g {O}><rect x="{x}" y="12" width="7" height="18" fill="url(#fn)"/></g><rect x="{x}" y="12" width="7" height="4" fill="#120c06"/>' for x in (20, 31, 42)) +
    f'<g {O}><rect x="12" y="28" width="42" height="7" fill="url(#hw)"/><rect x="8" y="34" width="50" height="6" fill="url(#hw)"/></g>'
    '<g fill="#3a5a8a">' + ''.join(f'<rect x="{x}" y="30" width="2.4" height="2.4"/>' for x in range(15, 52, 5)) + '</g>' +
    hull(2, 62, 40, 48, 54, bow_rake=6) +
    '<g fill="#e8e0d0">' + ''.join(f'<circle cx="{x}" cy="44" r="1"/>' for x in range(8, 56, 4)) + '</g>')
I['isle_craft_flying_boat'] = svg(DEFS + lg('pl', (0, '#e8eef4'), (.6, '#a8b4c0'), (1, '#5a6470')),
    sea() +
    f'<g {O}><path d="M4 16 L60 16 L58 21 L6 21 Z" fill="url(#pl)"/>'
    '<rect x="16" y="20" width="9" height="6" rx="2" fill="url(#ir)"/><rect x="39" y="20" width="9" height="6" rx="2" fill="url(#ir)"/>'
    '<path d="M8 34 C8 28 20 26 36 26 C48 26 56 30 58 36 L52 46 L14 46 C10 44 8 40 8 34 Z" fill="url(#pl)"/>'
    '<path d="M6 26 L12 24 L16 34 L10 34 Z" fill="url(#fn)"/></g>'
    '<g stroke="#2a1708" stroke-width="1.6"><path d="M22 21 L24 28 M42 21 L40 28"/></g>'
    '<g stroke="#2a1708" stroke-width="1.4"><path d="M20 26 L20 22 M44 26 L44 22"/></g>'
    '<g fill="#3a5a8a" stroke="#2a1708" stroke-width="1"><path d="M44 28 L52 30 L53 33 L44 32 Z"/></g>'
    '<path d="M12 40 L54 40" stroke="#c0503a" stroke-width="2"/>'
    '<path d="M12 31 C22 28 34 27 48 28" stroke="#fff" stroke-width="1.4" fill="none" opacity=".85"/>'
    f'<g {o(1.6)}><path d="M3 22 L3 30 L6 30 L6 22 Z M58 22 L58 30 L61 30 L61 22 Z" fill="url(#pl)"/></g>')

# Fire, riots and sickness: the watch, the constables, the infirmary, what a fire leaves, and
# the three alerts.
STONE = lg('st', (0, '#e8e0cc'), (1, '#a89a80'))
FLAME = lg('fl', (0, '#fff0a0'), (.4, '#ffb030'), (1, '#d0401a'))
I['isle_fire_watch'] = svg(DEFS + FLAME,
    shadow(32, 60, 20, 3) +
    f'<g {O}><path d="M20 58 L26 18 L38 18 L44 58 Z" fill="url(#wd)"/></g>'
    '<g stroke="#2a1708" stroke-width="1.6"><path d="M22 46 L42 46 M24 32 L40 32 M22 46 L40 32 M42 46 L24 32"/></g>'
    f'<g {O}><path d="M20 18 L44 18 L40 10 L24 10 Z" fill="url(#rl)"/><path d="M18 19 L46 19"/></g>'
    f'<g {o(1.8)}><path d="M28 18 C28 13 36 13 36 18 Z" fill="#f0c85a"/></g>'
    '<path d="M32 10 L32 2" stroke="#2a1708" stroke-width="1.8"/>'
    '<path d="M32 2 L44 5 L32 8 Z" fill="#d0401a" stroke="#2a1708" stroke-width="1.2"/>'
    f'<g {o(1.4)}><path d="M48 58 C44 52 48 46 50 42 C51 46 54 46 54 50 C56 48 57 46 56 44 C60 48 60 56 56 58 Z" fill="url(#fl)"/></g>'
    '<g fill="#4a6a8a" stroke="#2a1708" stroke-width="1.4"><path d="M8 50 L16 50 L15 58 L9 58 Z"/></g>'
    '<path d="M8 50 C8 46 16 46 16 50" stroke="#2a1708" stroke-width="1.4" fill="none"/>')
I['isle_watch_house'] = svg(DEFS + STONE + lg('bl', (0, '#6a8ab8'), (1, '#1a2a4a')),
    shadow(32, 60, 26, 3) +
    f'<g {O}><path d="M8 30 L8 58 L56 58 L56 30 Z" fill="url(#st)"/><path d="M4 32 L32 14 L60 32 Z" fill="url(#bl)"/></g>'
    f'<g {o(1.8)}><path d="M26 58 L26 40 C26 34 38 34 38 40 L38 58 Z" fill="url(#bl)"/>'
    '<rect x="12" y="38" width="9" height="9" fill="#ffd27a"/><rect x="43" y="38" width="9" height="9" fill="#ffd27a"/></g>'
    '<g stroke="#2a1708" stroke-width="1"><path d="M16.5 38 L16.5 47 M12 42.5 L21 42.5 M47.5 38 L47.5 47 M43 42.5 L52 42.5"/></g>'
    f'<g {o(1.8)}><path d="M28 22 L36 22 L35 30 L29 30 Z" fill="#9ad0ff"/><path d="M27 22 L37 22 L32 18 Z" fill="url(#bl)"/></g>'
    '<circle cx="32" cy="26" r="6" fill="#9ad0ff" opacity=".35"/>'
    '<path d="M32 18 L32 14" stroke="#2a1708" stroke-width="1.6"/>')
I['isle_infirmary'] = svg(DEFS + STONE,
    shadow(32, 60, 28, 3) +
    f'<g {O}><path d="M4 28 L4 58 L60 58 L60 28 Z" fill="url(#st)"/><path d="M0 30 L32 12 L64 30 Z" fill="url(#rl)"/></g>'
    f'<g {o(1.8)}><rect x="9" y="36" width="8" height="10" fill="#ffd27a"/><rect x="47" y="36" width="8" height="10" fill="#ffd27a"/>'
    '<path d="M27 58 L27 44 L37 44 L37 58 Z" fill="#4a2a14"/></g>'
    f'<g {o(1.8)}><circle cx="32" cy="32" r="8.5" fill="#f4f0e4"/></g>'
    f'<path d="M29.6 26.4 L34.4 26.4 L34.4 29.6 L37.6 29.6 L37.6 34.4 L34.4 34.4 L34.4 37.6 L29.6 37.6 L29.6 34.4 L26.4 34.4 L26.4 29.6 L29.6 29.6 Z" fill="#3a8a4a" {o(1.2)}/>')
I['isle_ruin'] = svg(DEFS + STONE,
    shadow(32, 60, 28, 3) +
    f'<g {O}><path d="M6 58 L6 30 L12 26 L16 34 L20 28 L22 58 Z" fill="url(#st)"/>'
    '<path d="M40 58 L40 36 L46 40 L50 30 L56 34 L58 58 Z" fill="url(#st)"/></g>'
    '<g fill="#2a1e14" opacity=".55"><path d="M6 40 L22 44 L22 58 L6 58 Z M40 46 L58 42 L58 58 L40 58 Z"/></g>'
    f'<g {O}><path d="M18 58 L44 46 L46 50 L22 60 Z" fill="#3a2a1a"/><path d="M26 58 L36 40 L39 42 L30 58 Z" fill="#2a1e14"/></g>'
    '<g fill="#6a6a68" opacity=".55"><circle cx="30" cy="30" r="5"/><circle cx="25" cy="21" r="4"/><circle cx="30" cy="12" r="3"/></g>'
    '<g fill="#ff8030"><circle cx="31" cy="54" r="1.2"/><circle cx="38" cy="50" r="1"/><circle cx="24" cy="56" r=".9"/></g>')
I['isle_alert_fire'] = svg(FLAME + lg('fi', (0, '#fff8d0'), (1, '#ffc040')),
    f'<g {o(2.4)}><path d="M32 4 C36 14 48 18 50 32 C52 46 44 58 32 58 C20 58 12 48 14 36 C15 28 20 24 22 18 C26 24 26 28 30 30 C30 22 28 12 32 4 Z" fill="url(#fl)"/></g>'
    f'<g {o(1.6)}><path d="M32 30 C36 36 42 40 40 48 C38 54 26 54 24 48 C22 42 28 38 32 30 Z" fill="url(#fi)"/></g>'
    '<path d="M20 38 C19 30 24 26 26 22" stroke="#fff4c0" stroke-width="1.8" fill="none" opacity=".8" stroke-linecap="round"/>')
I['isle_alert_riot'] = svg(DEFS + FLAME,
    f'<path d="M12 58 L46 12" {o(5.4)}/><path d="M12 58 L46 12" stroke="#b08050" stroke-width="2.8" stroke-linecap="round"/>'
    f'<g {o(2)} fill="none"><path d="M40 6 L48 18 M46 2 L54 14 M52 0 L58 10" stroke-width="4.4"/></g>'
    '<g stroke="#b8c0c8" stroke-width="2" fill="none" stroke-linecap="round"><path d="M40 6 L48 18 M46 2 L54 14 M52 0 L58 10"/></g>'
    f'<g {o(2.4)}><path d="M42 18 L54 10 L56 14 L46 22 Z" fill="url(#ir)"/></g>'
    f'<path d="M52 58 L20 18" {o(5.4)}/><path d="M52 58 L20 18" stroke="#8a5a2a" stroke-width="2.8" stroke-linecap="round"/>'
    f'<g {o(2)}><path d="M15 20 L25 14 L27 18 L17 24 Z" fill="#4a3018"/></g>'
    f'<g {o(1.8)}><path d="M20 15 C12 10 14 2 20 0 C20 4 24 4 24 8 C28 6 28 12 26 14 C24 16 22 16 20 15 Z" fill="url(#fl)"/></g>')
I['isle_alert_sick'] = svg(lg('mi', (0, '#d8f0a0'), (.5, '#8ab850'), (1, '#3a6a2a')) + lg('vl', (0, '#f4eee0'), (1, '#b8ae98')),
    f'<g {o(2.2)}><path d="M14 44 C6 44 4 34 12 32 C10 24 20 20 24 26 C26 16 40 16 42 26 C48 22 58 28 54 36 C60 40 56 48 50 46 C48 52 38 54 34 48 C28 54 18 52 14 44 Z" fill="url(#mi)"/></g>'
    '<g fill="#2a1708"><circle cx="24" cy="36" r="2.4"/><circle cx="40" cy="36" r="2.4"/></g>'
    '<path d="M24 46 C28 42 36 42 40 46" stroke="#2a1708" stroke-width="2.2" fill="none" stroke-linecap="round"/>'
    '<path d="M16 30 C18 26 22 25 24 27" stroke="#f0ffd0" stroke-width="1.6" fill="none" opacity=".8" stroke-linecap="round"/>'
    f'<g {o(1.6)}><path d="M48 50 C48 54 52 58 52 60 C52 63 44 63 44 60 C44 58 48 54 48 50 Z" fill="#8ab850"/></g>'
    f'<g {o(1.6)}><path d="M14 52 C14 55 17 58 17 60 C17 62 11 62 11 60 C11 58 14 55 14 52 Z" fill="#8ab850"/></g>')

# A brass compass rose: the "sail to" control.
def rose():
    import math
    pts = []
    for k in range(8):
        a = k * math.pi / 4
        r = 27 if k % 2 == 0 else 17
        b1, b2 = a - math.pi / 8, a + math.pi / 8
        tip = (32 + r * math.sin(a), 32 - r * math.cos(a))
        l = (32 + 6 * math.sin(b1), 32 - 6 * math.cos(b1))
        rr = (32 + 6 * math.sin(b2), 32 - 6 * math.cos(b2))
        light = '#fff0c0' if k % 2 == 0 else '#e8c880'
        dark = '#b0782a' if k % 2 == 0 else '#8a5a20'
        pts.append((k, tip, l, rr, light, dark))
    out = ''
    for k, tip, l, rr, light, dark in sorted(pts, key=lambda t: t[0] % 2 == 0):
        out += (f'<path d="M32 32 L{l[0]:.1f} {l[1]:.1f} L{tip[0]:.1f} {tip[1]:.1f} Z" fill="{light}" {o(1.0)}/>'
                f'<path d="M32 32 L{rr[0]:.1f} {rr[1]:.1f} L{tip[0]:.1f} {tip[1]:.1f} Z" fill="{dark}" {o(1.0)}/>')
    return out
I['isle_compass'] = svg(DEFS + lg('ring', (0, '#f0d090'), (.5, '#c08a3a'), (1, '#6a4416')),
    f'<g {o(2.4)}><circle cx="32" cy="32" r="22" fill="#f4e8c8"/><circle cx="32" cy="32" r="22" fill="none" stroke="url(#ring)" stroke-width="4"/></g>'
    '<circle cx="32" cy="32" r="24.2" fill="none" stroke="#2a1708" stroke-width="1.4"/>'
    '<circle cx="32" cy="32" r="19.6" fill="none" stroke="#2a1708" stroke-width="1"/>' +
    rose() +
    '<path d="M32 5 L32 32" stroke="#b03a24" stroke-width="0"/>'
    f'<path d="M32 32 L28.6 27 L32 5 Z" fill="#e0583a" {o(1.6)}/><path d="M32 32 L35.4 27 L32 5 Z" fill="#9a2a18" {o(1.6)}/>'
    f'<g {o(1.6)}><circle cx="32" cy="32" r="3.4" fill="url(#ring)"/></g>')

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
for n, s in I.items():
    open(os.path.join(out, n + '.svg'), 'w').write(s)
print(len(I))
