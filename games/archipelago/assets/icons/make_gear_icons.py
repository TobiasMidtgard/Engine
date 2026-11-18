# Painted-style icons for ship gear and captains (isle_gear_*), in the same hand as
# make_icons.py: warm dark outline, three-tone gradients, rim light, soft contact shadow.
# Usage: python3 make_gear_icons.py <out dir>
import os, sys
O = 'stroke="#2a1708" stroke-width="2.6" stroke-linejoin="round" stroke-linecap="round"'
def o(w):
    return f'stroke="#2a1708" stroke-width="{w}" stroke-linejoin="round" stroke-linecap="round"'
def lg(id, *stops, x1=0, y1=0, x2=0, y2=1):
    st = ''.join(f'<stop offset="{p}" stop-color="{c}"/>' for p, c in stops)
    return f'<linearGradient id="{id}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">{st}</linearGradient>'
def rg(id, *stops, cx=.4, cy=.35, r=.7):
    st = ''.join(f'<stop offset="{p}" stop-color="{c}"/>' for p, c in stops)
    return f'<radialGradient id="{id}" cx="{cx}" cy="{cy}" r="{r}">{st}</radialGradient>'
SH = '<radialGradient id="sh" cx=".5" cy=".5" r=".5"><stop offset="0" stop-color="#000" stop-opacity=".45"/><stop offset="1" stop-color="#000" stop-opacity="0"/></radialGradient>'
def shadow(cx=32, cy=56, rx=24, ry=5):
    return f'<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="url(#sh)"/>'
def svg(defs, body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs>{SH}{defs}</defs>{body}</svg>'
# Shared paints.
BRASS = lg('br', (0, '#fbe3a0'), (.45, '#d9a640'), (1, '#8a5a14'))
BRASS_H = lg('brh', (0, '#fbe3a0'), (.45, '#d9a640'), (1, '#8a5a14'), x2=1, y2=0)
IRON = lg('ir', (0, '#a8b0b8'), (.5, '#5f6870'), (1, '#2c3238'))
IRON_R = rg('irr', (0, '#c8d0d8'), (.5, '#6a737c'), (1, '#262b30'))
WOOD = lg('wd', (0, '#d8a46a'), (.55, '#9a6432'), (1, '#5a3414'))
NAVY = lg('nv', (0, '#5a7aaa'), (.5, '#2a4270'), (1, '#121f3a'))
CREAM = lg('cr', (0, '#fffaf0'), (.6, '#eadcb8'), (1, '#b8a47a'))
RED = lg('rd', (0, '#f08a6a'), (.5, '#c0402a'), (1, '#6a1a0e'))
I = {}

I['isle_gear_captain'] = svg(
    NAVY + BRASS + lg('vz', (0, '#4a4440'), (1, '#0a0806')) + lg('tp', (0, '#ffffff'), (1, '#d8d0c0')),
    shadow(32, 56, 24, 4) +
    f'<g {O}><path d="M6 30 C6 20 18 14 32 14 C46 14 58 20 58 30 C58 36 48 38 32 38 C16 38 6 36 6 30 Z" fill="url(#tp)"/>'
    '<path d="M12 32 C12 38 14 44 16 46 L48 46 C50 44 52 38 52 32 C44 36 20 36 12 32 Z" fill="url(#nv)"/>'
    '<path d="M14 46 C20 52 44 52 50 46 C46 56 18 56 14 46 Z" fill="url(#vz)"/></g>'
    '<path d="M15 44 L49 44" stroke="url(#br)" stroke-width="3"/><path d="M15 44 L49 44" stroke="#2a1708" stroke-width=".8" stroke-dasharray="2 2" opacity=".6"/>'
    f'<g {o(1.6)}><path d="M32 22 C27 22 25 27 26 31 C27 35 37 35 38 31 C39 27 37 22 32 22 Z" fill="url(#br)"/></g>'
    '<g stroke="#2a1708" stroke-width="1.5" fill="none" stroke-linecap="round"><path d="M32 24 L32 32 M29 26 L35 26 M28.5 30 C30 33 34 33 35.5 30"/></g>'
    '<path d="M10 27 C14 20 24 17 32 17" stroke="#fff" stroke-width="2" fill="none" opacity=".9"/>'
    '<path d="M18 49 C24 51 36 51 44 49" stroke="#8a8478" stroke-width="1.4" fill="none" opacity=".8"/>')

I['isle_gear_sails'] = svg(
    CREAM + WOOD + lg('cs', (0, '#f8efd8'), (1, '#c8b48a'), x2=1, y2=0),
    shadow(32, 60, 20, 3) +
    f'<g {O}><path d="M31 4 L33 4 L33 60 L31 60 Z" fill="url(#wd)"/>'
    '<path d="M12 10 L52 10 C56 18 56 24 52 30 L12 30 C8 24 8 18 12 10 Z" fill="url(#cr)"/>'
    '<path d="M10 34 L54 34 C59 43 59 50 54 56 L10 56 C5 50 5 43 10 34 Z" fill="url(#cs)"/></g>'
    f'<g {o(2.4)} fill="none"><path d="M8 10 L56 10 M6 34 L58 34"/></g>'
    '<g stroke="#a8946a" stroke-width="1.2" fill="none" opacity=".8"><path d="M22 11 C20 18 20 23 22 29 M42 11 C44 18 44 23 42 29 M22 35 C19 43 19 48 22 55 M42 35 C45 43 45 48 42 55"/></g>'
    '<path d="M14 13 C12 18 12 22 14 26 M12 37 C9 43 9 48 12 52" stroke="#fff" stroke-width="2" fill="none" opacity=".85"/>'
    '<path d="M33 4 L44 1 L33 7" fill="#c0402a" stroke="#2a1708" stroke-width="1.4" stroke-linejoin="round"/>')

I['isle_gear_sheathing'] = svg(
    lg('cu', (0, '#f6c08a'), (.45, '#c8783a'), (1, '#6a3414')) + WOOD + lg('sea', (0, '#5aa8d8'), (1, '#1a4a78')),
    shadow(32, 60, 26, 3) +
    f'<g {O}><path d="M4 14 L60 14 L60 22 L4 22 Z" fill="url(#wd)"/>'
    '<path d="M4 22 L60 22 C60 40 48 54 32 56 C16 54 4 40 4 22 Z" fill="url(#cu)"/></g>'
    '<g stroke="#5a2a0e" stroke-width="1.2" fill="none" opacity=".85"><path d="M5 30 C14 31 50 31 59 30 M8 38 C18 39 46 39 56 38 M13 46 C22 47 42 47 51 46"/>'
    '<path d="M16 22 L16 30 M30 22 L30 30 M44 22 L44 30 M10 30 L10 38 M23 30 L23 38 M37 30 L37 38 M51 30 L51 38 M17 38 L17 46 M30 38 L30 46 M44 38 L44 46 M24 46 L24 53 M38 46 L38 53"/></g>'
    '<g fill="#fde2b8">' + ''.join(f'<circle cx="{x}" cy="{y}" r=".9"/>' for x, y in ((12, 26), (22, 26), (36, 26), (50, 26), (16, 34), (30, 34), (44, 34), (24, 42), (37, 42))) + '</g>'
    '<path d="M7 25 C9 36 16 46 26 52" stroke="#ffe2b8" stroke-width="2" fill="none" opacity=".8"/>'
    '<path d="M4 18 L60 18" stroke="#2a1708" stroke-width="1" opacity=".5"/>')

I['isle_gear_propeller'] = svg(
    BRASS + rg('hb', (0, '#fff0c0'), (.5, '#d9a640'), (1, '#6a4410')),
    shadow(32, 58, 22, 4) +
    ''.join(f'<g transform="rotate({a} 32 30)"><path d="M32 30 C26 22 24 8 32 4 C42 6 42 20 32 30 Z" fill="url(#br)" {O}/>'
            f'<path d="M33 8 C38 10 38 18 34 24" stroke="#fff6d0" stroke-width="1.6" fill="none" opacity=".85"/></g>' for a in (0, 120, 240)) +
    f'<g {O}><circle cx="32" cy="30" r="7.5" fill="url(#hb)"/></g><circle cx="32" cy="30" r="2.4" fill="#2a1708"/>'
    '<path d="M27 27 C28 25 30 24 32 24" stroke="#fff" stroke-width="1.4" fill="none" opacity=".9"/>')

I['isle_gear_engine'] = svg(
    IRON + BRASS + rg('dl', (0, '#ffffff'), (.7, '#f0e6cc'), (1, '#b8a47a'), cx=.5, cy=.5, r=.5),
    shadow(32, 58, 24, 4) +
    f'<g {o(1.8)}><path d="M8 14 C3 14 3 7 9 7 C9 2 17 1 18 5 C22 2 28 5 25 10 C28 12 26 16 22 15 Z" fill="#f4f2ec"/></g>'
    '<path d="M9 9 C10 6 13 5 16 6" stroke="#fff" stroke-width="1.4" fill="none"/>'
    f'<g {O}><rect x="8" y="18" width="38" height="36" rx="10" fill="url(#ir)"/>'
    '<path d="M14 18 L14 13 L20 13 L20 18" fill="url(#br)"/></g>'
    '<g stroke="#1a1e22" stroke-width="1.2" opacity=".7"><path d="M8 28 L46 28 M8 44 L46 44"/></g>'
    '<g fill="#d0d6dc">' + ''.join(f'<circle cx="{x}" cy="{y}" r="1.1"/>' for x in (12, 20, 28, 36, 42) for y in (28, 44)) + '</g>'
    f'<g {O}><circle cx="44" cy="36" r="13" fill="url(#br)"/><circle cx="44" cy="36" r="9.5" fill="url(#dl)" stroke-width="1.6"/></g>'
    '<g stroke="#2a1708" stroke-width="1.1">' + ''.join(f'<path d="M44 27.5 L44 29.5" transform="rotate({a} 44 36)"/>' for a in range(-120, 121, 40)) + '</g>'
    '<path d="M44 36 L50 30" stroke="#c0402a" stroke-width="2.2" stroke-linecap="round"/><circle cx="44" cy="36" r="1.8" fill="#2a1708"/>'
    '<path d="M12 22 C10 30 10 40 12 48" stroke="#d8e0e8" stroke-width="2" fill="none" opacity=".7"/>')

I['isle_gear_coal'] = svg(
    IRON + rg('cl', (0, '#6a6a72'), (.5, '#2a2a30'), (1, '#0a0a0e')),
    shadow(32, 58, 24, 4) +
    f'<g {O}><path d="M8 30 L56 30 L50 56 L14 56 Z" fill="url(#ir)"/></g>' +
    ''.join(f'<g {o(1.8)}><path d="M{x-6} {y+2} L{x-3} {y-5} L{x+4} {y-6} L{x+7} {y} L{x+3} {y+5} L{x-3} {y+5} Z" fill="url(#cl)"/></g>'
            f'<path d="M{x-3} {y-4} L{x+3} {y-5}" stroke="#9a9aa4" stroke-width="1.2" opacity=".9"/>' for x, y in ((20, 27), (34, 24), (46, 28), (27, 17), (40, 15))) +
    '<path d="M8 30 L56 30" stroke="#2a1708" stroke-width="3"/>'
    '<path d="M14 34 L18 52" stroke="#c8d0d8" stroke-width="1.8" opacity=".6"/>'
    f'<g {o(2.2)} fill="none"><path d="M10 34 C2 34 2 46 12 46"/></g>')

I['isle_gear_hatch'] = svg(
    WOOD + lg('hd', (0, '#e8be86'), (1, '#8a5a2a')) + lg('cx', (0, '#e0b070'), (1, '#9a6430')),
    shadow(32, 58, 28, 4) +
    f'<g {O}><path d="M4 40 L32 26 L60 40 L32 54 Z" fill="url(#wd)"/><path d="M4 40 L4 45 L32 59 L32 54 Z" fill="#5a3414"/><path d="M60 40 L60 45 L32 59 L32 54 Z" fill="#7a4a20"/>'
    '<path d="M14 40 L32 31 L50 40 L32 49 Z" fill="#1a0e06"/></g>' +
    ''.join(f'<path d="M{14+k*4.5} {40-k*2.25} L{32+k*4.5} {49-k*2.25}" stroke="url(#hd)" stroke-width="1.8"/>' for k in range(1, 4)) +
    f'<g {O}><path d="M24 14 L36 8 L46 13 L34 19 Z" fill="url(#hd)"/><path d="M24 14 L24 28 L34 33 L34 19 Z" fill="url(#cx)"/><path d="M34 19 L46 13 L46 27 L34 33 Z" fill="#8a5a2a"/></g>'
    '<path d="M24 14 L34 33 M34 19 L24 28" stroke="#5a3414" stroke-width="1.2"/>'
    '<path d="M36 23 L40 35 L37 34 M40 35 L42 32" stroke="#2a1708" stroke-width="1.8" fill="none" stroke-linecap="round"/>'
    '<path d="M8 40 L30 29" stroke="#f6d6a0" stroke-width="1.6" opacity=".8"/>')

I['isle_gear_overboard'] = svg(
    lg('wv', (0, '#8acbf0'), (.5, '#3a86c0'), (1, '#16406a')) + lg('cx', (0, '#e0b070'), (1, '#9a6430')),
    f'<g {O}><path d="M2 46 C10 40 16 50 24 44 C32 38 38 50 46 44 C52 40 58 44 62 42 L62 62 L2 62 Z" fill="url(#wv)"/></g>'
    f'<g transform="rotate(18 32 26)"><g {O}><rect x="20" y="14" width="24" height="22" rx="1.5" fill="url(#cx)"/></g>'
    '<path d="M20 14 L44 36 M44 14 L20 36" stroke="#6a4420" stroke-width="1.6"/><path d="M20 25 L44 25" stroke="#6a4420" stroke-width="1.6"/>'
    '<path d="M22 16 L42 16" stroke="#f6d6a0" stroke-width="1.6" opacity=".8"/></g>'
    '<g fill="#ffffff" stroke="#2a1708" stroke-width="1.6" stroke-linejoin="round"><path d="M14 46 L10 38 L17 43 L18 34 L22 43 Z"/><path d="M42 44 L46 35 L46 43 L52 37 L50 45 Z"/></g>'
    '<path d="M8 50 C14 47 18 52 24 49 M38 50 C44 47 48 52 54 49" stroke="#d8f0ff" stroke-width="1.6" fill="none" opacity=".8"/>'
    '<path d="M50 8 C54 10 56 14 54 18 M54 6 C60 10 60 18 57 22" stroke="#2a1708" stroke-width="2" fill="none" stroke-linecap="round" opacity=".55"/>')

I['isle_gear_chronometer'] = svg(
    BRASS + rg('fc', (0, '#ffffff'), (.75, '#f4ead0'), (1, '#c8b48a'), cx=.45, cy=.4, r=.6),
    shadow(32, 58, 20, 4) +
    f'<g {O}><path d="M28 6 L36 6 L36 12 L28 12 Z" fill="url(#br)"/><circle cx="32" cy="6" r="4" fill="none" stroke-width="2.4"/>'
    '<circle cx="32" cy="35" r="22" fill="url(#br)"/><circle cx="32" cy="35" r="17" fill="url(#fc)" stroke-width="1.8"/></g>'
    '<g stroke="#2a1708" stroke-width="1.6" stroke-linecap="round">' + ''.join(f'<path d="M32 20 L32 {23 if a % 90 == 0 else 22}" transform="rotate({a} 32 35)"/>' for a in range(0, 360, 30)) + '</g>'
    f'<g {o(1.2)} fill="none"><circle cx="32" cy="44" r="4"/></g><path d="M32 44 L32 41" stroke="#2a1708" stroke-width="1"/>'
    '<path d="M32 35 L32 24 M32 35 L40 39" stroke="#2a1708" stroke-width="2.4" stroke-linecap="round"/><path d="M32 35 L23 30" stroke="#c0402a" stroke-width="1.2"/>'
    '<circle cx="32" cy="35" r="2" fill="#2a1708"/>'
    '<path d="M15 26 C18 19 24 15 31 14" stroke="#fff6d0" stroke-width="2" fill="none" opacity=".9"/>')

I['isle_gear_spyglass'] = svg(
    BRASS + lg('lt', (0, '#8a4a2a'), (1, '#3a1a0a'), x2=1, y2=0) + rg('ln', (0, '#e8f6ff'), (.6, '#6ab0e0'), (1, '#1a4a78')),
    shadow(32, 56, 26, 4) +
    f'<g transform="rotate(-30 32 32)"><g {O}><rect x="2" y="27" width="18" height="10" rx="2" fill="url(#lt)"/>'
    '<rect x="18" y="25" width="16" height="14" rx="2" fill="url(#br)"/><rect x="32" y="23" width="16" height="18" rx="2" fill="url(#br)"/>'
    '<rect x="46" y="21" width="12" height="22" rx="3" fill="url(#br)"/><ellipse cx="58" cy="32" rx="3" ry="10" fill="url(#ln)"/></g>'
    '<g stroke="#8a5a14" stroke-width="1.4"><path d="M20 27 L20 37 M34 25 L34 39 M48 23 L48 41"/></g>'
    '<path d="M20 27.5 L32 27.5 M34 25.5 L46 25.5 M48 23.5 L56 23.5" stroke="#fff6d0" stroke-width="1.6" opacity=".9"/></g>'
    '<path d="M54 10 L57 4 M58 14 L63 11 M52 6 L52 1" stroke="#f0c85a" stroke-width="1.6" stroke-linecap="round"/>')

I['isle_gear_gas_cell'] = svg(
    lg('gc', (0, '#f4eee0'), (.5, '#c8bea6'), (1, '#7a6e58')) + WOOD,
    shadow(32, 60, 18, 3) +
    f'<g {O}><path d="M4 26 C4 12 18 6 32 6 C46 6 60 12 60 26 C60 38 46 44 32 44 C18 44 4 38 4 26 Z" fill="url(#gc)"/></g>'
    '<g stroke="#6a5a40" stroke-width="1.1" fill="none" opacity=".75"><path d="M16 9 C12 20 12 32 16 42 M32 6 L32 44 M48 9 C52 20 52 32 48 42"/>'
    '<path d="M5 22 C20 26 44 26 59 22 M6 32 C20 36 44 36 58 32"/></g>'
    '<g stroke="#2a1708" stroke-width="1.6" fill="none"><path d="M14 40 L24 52 M50 40 L40 52 M32 44 L32 52"/></g>'
    f'<g {O}><path d="M20 52 L44 52 L42 58 L22 58 Z" fill="url(#wd)"/></g>'
    '<path d="M10 20 C14 13 22 10 30 9" stroke="#fff" stroke-width="2.4" fill="none" opacity=".9"/>')

I['isle_gear_radio'] = svg(
    WOOD + BRASS + lg('pn', (0, '#3a3430'), (1, '#121010')),
    shadow(32, 60, 24, 3) +
    '<g stroke="#2a1708" stroke-width="2" fill="none" stroke-linecap="round"><path d="M44 22 L50 4"/></g>'
    '<g stroke="#f0c85a" stroke-width="1.8" fill="none" stroke-linecap="round" opacity=".9"><path d="M44 6 C42 3 44 0 47 -1 M56 8 C58 4 56 0 53 -1"/><path d="M41 10 C37 4 41 -3 47 -4 M59 12 C63 6 59 -2 53 -3"/></g>'
    f'<g {O}><rect x="6" y="22" width="52" height="34" rx="4" fill="url(#wd)"/><rect x="11" y="27" width="26" height="24" rx="3" fill="url(#pn)"/></g>'
    '<g stroke="#8a7a64" stroke-width="1.2" opacity=".8"><path d="M14 31 L34 31 M14 35 L34 35 M14 39 L34 39 M14 43 L34 43 M14 47 L34 47"/></g>'
    f'<g {o(1.8)}><circle cx="48" cy="33" r="5.5" fill="url(#br)"/><circle cx="48" cy="47" r="4" fill="url(#br)"/></g>'
    '<path d="M48 33 L51 30" stroke="#2a1708" stroke-width="1.6"/>'
    '<path d="M9 25 L55 25" stroke="#f6d6a0" stroke-width="1.6" opacity=".7"/>')

I['isle_gear_mail'] = svg(
    CREAM + RED,
    shadow(32, 56, 26, 4) +
    f'<g {O}><path d="M4 16 L60 16 L60 50 L4 50 Z" fill="url(#cr)"/></g>'
    f'<g {o(2)} fill="none"><path d="M4 16 L32 36 L60 16 M4 50 L24 31 M60 50 L40 31"/></g>'
    f'<g {o(1.8)}><path d="M32 30 C26 30 24 36 26 40 C28 45 36 45 38 40 C40 36 38 30 32 30 Z" fill="url(#rd)"/></g>'
    '<path d="M29 35 C30 33 32 33 33 34 L31 40" stroke="#ffb8a0" stroke-width="1.4" fill="none"/>'
    '<path d="M8 20 L28 34" stroke="#fff" stroke-width="1.8" opacity=".8"/>'
    '<g fill="#3a5a9a" stroke="#2a1708" stroke-width="1.2"><rect x="48" y="20" width="8" height="9"/></g>')

I['isle_gear_cat'] = svg(
    lg('ct', (0, '#4a4440'), (.6, '#24201c'), (1, '#0a0806')) + lg('wh', (0, '#ffffff'), (1, '#d8d0c0')),
    shadow(32, 60, 20, 3) +
    f'<g {O}><path d="M48 58 C60 56 62 44 54 40 C50 38 46 42 50 46" fill="none" stroke-width="7"/></g>'
    '<path d="M48 58 C60 56 62 44 54 40 C50 38 46 42 50 46" fill="none" stroke="#3a3430" stroke-width="3.2"/>'
    f'<g {O}><path d="M16 58 C12 44 16 32 22 28 L42 28 C48 32 52 44 48 58 Z" fill="url(#ct)"/>'
    '<path d="M18 30 C14 18 18 8 20 4 L28 14 L36 14 L44 4 C46 8 50 18 46 30 C40 36 24 36 18 30 Z" fill="url(#ct)"/>'
    '<path d="M26 34 C26 44 28 54 32 58 C36 54 38 44 38 34 C34 36 30 36 26 34 Z" fill="url(#wh)"/></g>'
    '<g fill="#d8c040" stroke="#2a1708" stroke-width="1.2"><ellipse cx="25" cy="22" rx="3.4" ry="3"/><ellipse cx="39" cy="22" rx="3.4" ry="3"/></g>'
    '<g fill="#0a0806"><ellipse cx="25" cy="22" rx="1" ry="2.4"/><ellipse cx="39" cy="22" rx="1" ry="2.4"/></g>'
    '<path d="M30 27 L34 27 L32 29 Z" fill="#e89a9a"/>'
    '<g stroke="#e8e0d0" stroke-width=".9" opacity=".8"><path d="M27 28 L18 27 M27 29.5 L18 31 M37 28 L46 27 M37 29.5 L46 31"/></g>'
    '<path d="M21 8 L22 14 M43 8 L42 14" stroke="#c07a7a" stroke-width="1.6"/>'
    '<path d="M20 34 C17 42 17 50 19 56" stroke="#7a7470" stroke-width="1.6" fill="none" opacity=".8"/>')

I['isle_gear_repair'] = svg(
    WOOD + IRON + lg('hd2', (0, '#c8945a'), (1, '#6a4420'), x2=1, y2=0),
    shadow(32, 58, 26, 4) +
    f'<g {O}><path d="M6 40 L58 30 L60 40 L8 50 Z" fill="url(#wd)"/></g>'
    '<g fill="#3a2414">' + ''.join(f'<circle cx="{x}" cy="{y}" r="1.3"/>' for x, y in ((12, 43), (24, 41), (40, 38), (54, 35))) + '</g>'
    f'<g transform="rotate(40 32 26)"><g {O}><rect x="29" y="12" width="6" height="34" rx="2" fill="url(#hd2)"/><path d="M20 6 L44 6 L44 14 L20 14 Z" fill="url(#ir)"/></g>'
    '<path d="M22 8 L42 8" stroke="#e0e8f0" stroke-width="1.4" opacity=".85"/></g>'
    f'<g transform="rotate(-40 32 26)"><g {O}><rect x="29.5" y="12" width="5" height="32" rx="2" fill="url(#ir)"/>'
    '<path d="M26 4 C26 12 38 12 38 4 L35 4 L35 8 L29 8 L29 4 Z" fill="url(#ir)"/></g></g>')

I['isle_gear_flare'] = svg(
    RED + WOOD + rg('st', (0, '#ffffff'), (.35, '#fff0a0'), (1, '#ff8a2a'), cx=.5, cy=.5, r=.5),
    shadow(22, 60, 12, 3) +
    '<path d="M20 58 C22 46 28 34 40 20" stroke="#ffb060" stroke-width="5" fill="none" opacity=".35" stroke-linecap="round"/>'
    '<path d="M20 58 C22 46 28 34 40 20" stroke="#ffe0a0" stroke-width="1.6" fill="none" stroke-dasharray="3 3" stroke-linecap="round"/>'
    '<g stroke="#ffd27a" stroke-width="2.4" stroke-linecap="round">' + ''.join(f'<path d="M46 14 L{46+12*c} {14+12*s}"/>' for c, s in ((1, 0), (.7, .7), (0, 1), (-.7, .7), (-1, 0), (-.7, -.7), (0, -1), (.7, -.7))) + '</g>'
    '<circle cx="46" cy="14" r="8" fill="url(#st)"/>'
    f'<g transform="rotate(35 16 46)"><g {O}><path d="M12 36 L20 36 L20 54 L12 54 Z" fill="url(#rd)"/><path d="M12 36 L16 28 L20 36 Z" fill="#e8e0d0"/></g>'
    '<path d="M16 54 L16 62" stroke="#2a1708" stroke-width="2.4"/><path d="M14 38 L14 52" stroke="#ffc8b0" stroke-width="1.4" opacity=".8"/></g>')

I['isle_gear_purse'] = svg(
    lg('le', (0, '#c8945a'), (.5, '#8a5a2a'), (1, '#4a2a10')) + BRASS + rg('cn', (0, '#fff0b0'), (.6, '#e0b040'), (1, '#8a5a10')),
    shadow(32, 58, 26, 4) +
    f'<g {O}><path d="M14 26 C6 34 6 52 18 56 L46 56 C58 52 58 34 50 26 Z" fill="url(#le)"/>'
    '<path d="M18 26 C20 18 24 14 32 14 C40 14 44 18 46 26 Z" fill="url(#le)"/></g>'
    f'<g {o(2.4)} fill="none"><path d="M16 26 C24 30 40 30 48 26"/></g><path d="M16 26 C24 30 40 30 48 26" stroke="url(#br)" stroke-width="1.4" fill="none"/>'
    f'<g {o(1.8)}><circle cx="32" cy="12" r="4" fill="url(#br)"/></g>' +
    ''.join(f'<g {o(1.6)}><ellipse cx="{x}" cy="{y}" rx="6" ry="3.4" fill="url(#cn)"/></g>' for x, y in ((48, 58), (56, 55), (52, 51))) +
    '<path d="M14 34 C11 42 12 48 16 52" stroke="#e8be86" stroke-width="2" fill="none" opacity=".75"/>'
    '<path d="M28 40 L36 40 M32 36 L32 46" stroke="#4a2a10" stroke-width="1.4" opacity=".7"/>')

I['isle_gear_flag'] = svg(
    RED + WOOD + BRASS,
    shadow(16, 60, 10, 3) +
    f'<g {O}><path d="M12 6 L16 6 L16 60 L12 60 Z" fill="url(#wd)"/><circle cx="14" cy="5" r="3.4" fill="url(#br)"/>'
    '<path d="M16 10 C26 6 34 14 44 10 C50 8 56 10 60 12 C56 20 56 28 60 34 C54 32 48 34 42 36 C32 40 24 32 16 36 Z" fill="url(#rd)"/></g>'
    '<path d="M16 22 C26 18 34 26 44 22 C50 20 54 22 58 23" stroke="#fff4e0" stroke-width="4" fill="none"/>'
    '<path d="M16 22 C26 18 34 26 44 22 C50 20 54 22 58 23" stroke="#2a1708" stroke-width="1" fill="none" opacity=".25"/>'
    '<path d="M18 12 C26 9 32 15 40 12" stroke="#ffb8a0" stroke-width="1.6" fill="none" opacity=".85"/>')

I['isle_gear_cannon'] = svg(
    IRON + WOOD + IRON_R,
    shadow(32, 58, 28, 4) +
    f'<g {O}><path d="M6 24 L50 14 C54 13 56 16 56 19 L56 27 C56 30 54 32 50 31 L8 36 C4 36 3 26 6 24 Z" fill="url(#ir)"/>'
    '<ellipse cx="56" cy="23" rx="3" ry="6" fill="#121416"/></g>'
    '<g stroke="#2a1708" stroke-width="1.6"><path d="M20 21 L21 34 M38 17 L39 32"/></g>'
    '<path d="M10 26 L50 17" stroke="#d0d8e0" stroke-width="1.8" opacity=".8"/>'
    f'<g {O}><path d="M12 36 L44 32 L46 44 L14 50 Z" fill="url(#wd)"/><circle cx="18" cy="50" r="7" fill="url(#wd)"/><circle cx="40" cy="46" r="7" fill="url(#wd)"/></g>'
    '<g fill="#2a1708"><circle cx="18" cy="50" r="2"/><circle cx="40" cy="46" r="2"/></g>'
    f'<g {o(1.6)}><circle cx="5" cy="30" r="3.4" fill="url(#ir)"/></g>')

I['isle_gear_shot'] = svg(
    IRON_R,
    shadow(32, 58, 26, 4) +
    ''.join(f'<g {O}><circle cx="{x}" cy="{y}" r="11" fill="url(#irr)"/></g><path d="M{x-6} {y-4} C{x-4} {y-8} {x} {y-9} {x+2} {y-9}" stroke="#eef2f6" stroke-width="2" fill="none" opacity=".85"/>'
            for x, y in ((20, 46), (44, 46), (32, 26))))

I['isle_gear_armour'] = svg(
    IRON + lg('ir2', (0, '#8a929a'), (1, '#3a4046'), x2=1, y2=0),
    shadow(32, 60, 22, 3) +
    f'<g {O}><path d="M8 8 L56 8 L56 30 C56 44 44 54 32 60 C20 54 8 44 8 30 Z" fill="url(#ir)"/>'
    '<path d="M14 14 L50 14 L50 30 C50 40 42 48 32 53 C22 48 14 40 14 30 Z" fill="url(#ir2)" stroke-width="1.8"/></g>'
    '<g fill="#d8e0e8" stroke="#2a1708" stroke-width="1">' + ''.join(f'<circle cx="{x}" cy="{y}" r="1.8"/>' for x, y in ((11, 11), (32, 11), (53, 11), (11, 30), (53, 30), (20, 46), (44, 46), (32, 56))) + '</g>'
    '<path d="M14 30 L50 30 M32 14 L32 53" stroke="#2a1708" stroke-width="1.2" opacity=".5"/>'
    '<path d="M12 12 L30 12 M12 14 L12 30" stroke="#eef2f6" stroke-width="1.8" opacity=".75"/>')

I['isle_gear_ram'] = svg(
    WOOD + IRON + lg('sea', (0, '#5aa8d8'), (1, '#1a4a78')),
    f'<g {O}><path d="M2 50 C12 46 22 54 32 50 C42 46 52 54 62 50 L62 62 L2 62 Z" fill="url(#sea)"/></g>'
    f'<g {O}><path d="M30 6 L44 6 C44 22 42 36 36 48 L14 48 C14 34 20 18 30 6 Z" fill="url(#wd)"/>'
    '<path d="M34 34 L60 42 L34 48 C32 44 32 38 34 34 Z" fill="url(#ir)"/></g>'
    '<path d="M36 38 L56 42" stroke="#e0e8f0" stroke-width="1.6" opacity=".85"/>'
    '<g stroke="#5a3414" stroke-width="1.2" opacity=".8"><path d="M22 20 L42 20 M18 30 L42 30 M16 40 L36 40"/></g>'
    '<path d="M31 9 C25 18 20 30 18 44" stroke="#f6d6a0" stroke-width="1.8" fill="none" opacity=".8"/>')

I['isle_gear_sights'] = svg(
    BRASS + rg('gl', (0, '#e8f6ff'), (.6, '#8ac0e0'), (1, '#2a5a80'), cx=.45, cy=.4, r=.6),
    shadow(32, 60, 18, 3) +
    f'<g {O}><circle cx="32" cy="30" r="24" fill="url(#br)"/><circle cx="32" cy="30" r="18" fill="url(#gl)" stroke-width="1.8"/></g>'
    '<g stroke="#2a1708" stroke-width="1.8" stroke-linecap="round"><path d="M32 13 L32 24 M32 36 L32 47 M15 30 L26 30 M38 30 L49 30"/></g>'
    '<circle cx="32" cy="30" r="5" fill="none" stroke="#c0402a" stroke-width="1.6"/><circle cx="32" cy="30" r="1.2" fill="#c0402a"/>'
    '<g stroke="#2a1708" stroke-width="1">' + ''.join(f'<path d="M32 13 L32 15.5" transform="rotate({a} 32 30)"/>' for a in range(15, 360, 30)) + '</g>'
    '<path d="M18 22 C21 16 26 13 31 12.5" stroke="#fff" stroke-width="2" fill="none" opacity=".85"/>')

I['isle_gear_bomb'] = svg(
    rg('bm', (0, '#7a8088'), (.5, '#2a2e34'), (1, '#0a0c0e')) + IRON + rg('sp', (0, '#ffffff'), (.4, '#ffe07a'), (1, '#ff7a1a00'), cx=.5, cy=.5, r=.5),
    shadow(30, 60, 22, 3) +
    f'<g {O}><circle cx="28" cy="38" r="20" fill="url(#bm)"/><path d="M36 16 L44 14 L46 22 L38 24 Z" fill="url(#ir)"/></g>'
    '<path d="M45 16 C50 12 52 8 50 4" stroke="#2a1708" stroke-width="2.6" fill="none" stroke-linecap="round"/><path d="M45 16 C50 12 52 8 50 4" stroke="#c8a46a" stroke-width="1.2" fill="none"/>'
    '<circle cx="50" cy="4" r="7" fill="url(#sp)"/>'
    '<g stroke="#ffd27a" stroke-width="1.6" stroke-linecap="round"><path d="M50 4 L57 1 M50 4 L56 9 M50 4 L44 -1 M50 4 L58 5"/></g>'
    '<path d="M16 30 C18 24 24 20 30 20" stroke="#c8d0d8" stroke-width="2.4" fill="none" opacity=".75"/>')

I['isle_gear_cutlass'] = svg(
    lg('bl', (0, '#f4f8fc'), (.5, '#a8b4c0'), (1, '#5a6470'), x2=1, y2=0) + BRASS + lg('gp', (0, '#6a3a1a'), (1, '#2a1408')),
    shadow(32, 60, 22, 3) +
    f'<g {O}><path d="M20 44 C24 30 34 14 54 4 C50 18 40 34 26 48 Z" fill="url(#bl)"/>'
    '<path d="M10 44 C14 40 24 40 30 48 C26 50 22 52 18 50 C16 54 12 56 10 54 C8 52 8 48 10 44 Z" fill="url(#br)"/>'
    '<path d="M14 52 L8 58 L12 62 L18 56 Z" fill="url(#gp)"/></g>'
    '<path d="M24 42 C28 32 36 20 50 9" stroke="#ffffff" stroke-width="1.6" fill="none" opacity=".9"/>'
    f'<g {o(1.6)}><circle cx="7" cy="60" r="3" fill="url(#br)"/></g>')

I['isle_gear_smoke'] = svg(
    rg('sm', (0, '#f0f0ec'), (.6, '#a8a8a4'), (1, '#5a5a58')) + rg('sm2', (0, '#d8d8d4'), (.6, '#8a8a86'), (1, '#4a4a48')),
    shadow(32, 60, 26, 3) +
    f'<g {O}><path d="M8 50 C2 50 2 40 10 39 C8 30 18 26 24 31 C26 22 40 22 42 30 C50 26 60 32 56 40 C62 42 60 52 52 52 Z" fill="url(#sm2)"/>'
    '<path d="M16 34 C12 28 16 20 24 22 C24 12 38 10 40 20 C48 16 56 24 50 32 C44 34 24 36 16 34 Z" fill="url(#sm)"/></g>'
    '<path d="M20 24 C22 18 28 16 32 17 M42 22 C46 20 50 22 51 25" stroke="#fff" stroke-width="2" fill="none" opacity=".85"/>'
    '<path d="M28 6 C26 2 30 0 32 2 M38 8 C38 4 42 4 42 7" stroke="#8a8a86" stroke-width="2.4" fill="none" stroke-linecap="round"/>')

I['isle_gear_horseshoe'] = svg(
    IRON,
    shadow(32, 60, 20, 3) +
    f'<g {O}><path d="M12 54 L12 30 C12 14 22 6 32 6 C42 6 52 14 52 30 L52 54 L42 54 L42 30 C42 22 38 16 32 16 C26 16 22 22 22 30 L22 54 Z" fill="url(#ir)"/>'
    '<path d="M10 54 L24 54 L24 58 L10 58 Z M40 54 L54 54 L54 58 L40 58 Z" fill="#3a4046"/></g>'
    '<g fill="#1a1e22">' + ''.join(f'<rect x="{x-1.2}" y="{y-2}" width="2.4" height="4" rx="1"/>' for x, y in ((17, 44), (16.5, 32), (19, 20), (47, 44), (47.5, 32), (45, 20))) + '</g>'
    '<path d="M15 44 L15 30 C15 18 22 10 30 9" stroke="#e0e8f0" stroke-width="2" fill="none" opacity=".75"/>'
    '<g stroke="#f0c85a" stroke-width="1.6" stroke-linecap="round"><path d="M58 8 L62 4 M58 4 L62 8 M6 10 L4 6"/></g>')

I['isle_gear_torpedo'] = svg(
    lg('tp2', (0, '#d0d8e0'), (.45, '#7a848e'), (1, '#2a3036')) + BRASS + RED,
    shadow(32, 52, 28, 4) +
    '<g transform="translate(32 32) rotate(-14) scale(1.12) translate(-32 -32)">' +
    f'<g {O}><path d="M10 26 L46 26 L46 38 L10 38 C4 38 4 26 10 26 Z" fill="url(#tp2)"/>'
    '<path d="M4 32 C4 28 6 26 10 26 L14 26 L14 38 L10 38 C6 38 4 36 4 32 Z" fill="url(#rd)"/>'
    '<path d="M46 26 L52 28 L52 36 L46 38 Z" fill="url(#br)"/></g>'
    f'<g {o(2)}><path d="M52 32 L58 22 L60 24 L55 32 L60 40 L58 42 Z" fill="url(#br)"/></g>'
    '<path d="M16 28 L44 28" stroke="#fff" stroke-width="1.6" opacity=".85"/>'
    '<g stroke="#2a1708" stroke-width="1.2"><path d="M24 26 L24 38 M36 26 L36 38"/></g>'
    '<path d="M40 22 L46 26 M40 42 L46 38" stroke="#2a1708" stroke-width="2.2"/></g>')

I['isle_gear_mine'] = svg(
    rg('mn', (0, '#8a929a'), (.5, '#3a4046'), (1, '#121416')) + IRON + lg('sea', (0, '#5aa8d8'), (1, '#1a4a78')),
    f'<g {O}><path d="M2 50 C12 46 22 54 32 50 C42 46 52 54 62 50 L62 62 L2 62 Z" fill="url(#sea)"/></g>'
    '<g stroke="#2a1708" stroke-width="5" stroke-linecap="round">' + ''.join(f'<path d="M32 30 L{32+21*c:.1f} {30+21*s:.1f}"/>' for c, s in ((0, -1), (.87, -.5), (.87, .5), (-.87, .5), (-.87, -.5), (.5, -.87), (-.5, -.87))) + '</g>'
    '<g stroke="#8a929a" stroke-width="2.2" stroke-linecap="round">' + ''.join(f'<path d="M32 30 L{32+20*c:.1f} {30+20*s:.1f}"/>' for c, s in ((0, -1), (.87, -.5), (.87, .5), (-.87, .5), (-.87, -.5), (.5, -.87), (-.5, -.87))) + '</g>'
    f'<g {O}><circle cx="32" cy="30" r="15" fill="url(#mn)"/></g>'
    '<path d="M20 26 C22 20 26 17 31 16" stroke="#d8e0e8" stroke-width="2" fill="none" opacity=".8"/>'
    '<path d="M18 33 L46 33" stroke="#2a1708" stroke-width="1.4" opacity=".6"/>')

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
for n, s in I.items():
    open(os.path.join(out, n + '.svg'), 'w').write(s)
print(len(I))
