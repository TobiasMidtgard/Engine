# Painted-style icons for every craft, the shipyard and the ship chandler (isle_craft_*,
# isle_shipyard, isle_chandler, isle_compass), in the same hand as make_icons.py.
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
