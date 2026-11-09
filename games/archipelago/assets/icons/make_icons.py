# Painted-style icons for Archipelago (64x64): warm dark outline, three-tone gradients,
# rim highlights and a soft contact shadow. Writes SVG files and a contact sheet.
import os, sys
O = 'stroke="#2a1708" stroke-width="2.6" stroke-linejoin="round" stroke-linecap="round"'
def lg(id, *stops, x1=0, y1=0, x2=0, y2=1):
    st = ''.join(f'<stop offset="{o}" stop-color="{c}"/>' for o, c in stops)
    return f'<linearGradient id="{id}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">{st}</linearGradient>'
def rg(id, *stops, cx=.4, cy=.35, r=.7):
    st = ''.join(f'<stop offset="{o}" stop-color="{c}"/>' for o, c in stops)
    return f'<radialGradient id="{id}" cx="{cx}" cy="{cy}" r="{r}">{st}</radialGradient>'
SH = rg('sh', (0, '#000000'), (1, '#00000000'), cx=.5, cy=.5, r=.5).replace('<stop offset="0" stop-color="#000000"/>', '<stop offset="0" stop-color="#000" stop-opacity=".45"/>').replace('<stop offset="1" stop-color="#00000000"/>', '<stop offset="1" stop-color="#000" stop-opacity="0"/>')
def shadow(cx=32, cy=56, rx=24, ry=5):
    return f'<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="url(#sh)"/>'
def svg(defs, body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs>{SH}{defs}</defs>{body}</svg>'
I = {}
I['isle_logs'] = svg(
    lg('bk', (0, '#b07a44'), (.5, '#7a4a22'), (1, '#4a2a10')) + rg('ct', (0, '#fbe3b0'), (.6, '#e2b77a'), (1, '#b07a3e'), cx=.45, cy=.45, r=.6),
    shadow(32, 54, 26, 5) +
    f'<g {O}><path d="M14 52 L50 36 L56 46 L20 62 Z" transform="translate(0,-8)" fill="url(#bk)"/>'
    '<ellipse cx="17" cy="49" rx="6.5" ry="7.5" fill="url(#ct)"/>'
    '<path d="M18 33 L50 18 L56 28 L24 43 Z" fill="url(#bk)"/><ellipse cx="21" cy="38" rx="6" ry="7" fill="url(#ct)"/>'
    '<path d="M30 46 L58 33 L62 41 L36 53 Z" fill="url(#bk)"/></g>'
    '<g fill="none" stroke="#9a6a38" stroke-width="1"><circle cx="17" cy="49" r="3.4"/><circle cx="17" cy="49" r="1.3"/><circle cx="21" cy="38" r="3"/><circle cx="21" cy="38" r="1.1"/></g>'
    '<path d="M24 40 L48 29 M28 52 L50 42" stroke="#c9945a" stroke-width="1.4" opacity=".7"/>'
    '<path d="M22 31 L48 19" stroke="#e8b47a" stroke-width="1.6" opacity=".8"/>'
    '<path d="M52 20 C56 16 60 18 58 22" fill="#5f9a3a" stroke="#2a1708" stroke-width="1.6"/>')
I['isle_planks'] = svg(
    lg('pl', (0, '#f6d8a0'), (.55, '#d3a265'), (1, '#9a6a36')) + lg('pe', (0, '#b07a44'), (1, '#6a4420')),
    shadow(32, 55, 27, 5) +
    ''.join(f'<g {O}><path d="M6 {y+8} L38 {y+18} L38 {y+23} L6 {y+13} Z" fill="url(#pe)"/><path d="M38 {y+18} L58 {y+8} L58 {y+13} L38 {y+23} Z" fill="url(#pe)"/><path d="M6 {y+8} L26 {y-2} L58 {y+8} L38 {y+18} Z" fill="url(#pl)"/></g>'
            f'<path d="M14 {y+6} L36 {y+13} M22 {y+2} L44 {y+9}" stroke="#a87440" stroke-width="1.1" opacity=".75"/>' for y in (36, 24, 12)) +
    '<path d="M27 11 L54 19" stroke="#fff3d2" stroke-width="1.4" opacity=".8"/>')
I['isle_wool'] = svg(
    rg('wl', (0, '#ffffff'), (.6, '#efe8d8'), (1, '#b9ae94')),
    shadow(32, 54, 24, 5) +
    f'<g {O}><path d="M14 46 C5 46 5 33 13 32 C11 22 23 17 29 23 C33 14 48 15 48 26 C58 25 60 38 51 42 C51 51 40 53 36 48 C31 55 17 54 14 46 Z" fill="url(#wl)"/></g>'
    '<g fill="none" stroke="#b8ad92" stroke-width="1.6" stroke-linecap="round"><path d="M19 38 C21 34 25 34 26 37"/><path d="M32 30 C34 26 39 26 40 30"/><path d="M36 42 C38 38 43 38 44 42"/><path d="M24 46 C26 43 30 43 31 46"/></g>'
    '<path d="M16 32 C17 27 22 25 26 27" fill="none" stroke="#fff" stroke-width="2" opacity=".9"/>'
    '<path d="M50 40 C56 44 58 50 54 56" fill="none" stroke="#2a1708" stroke-width="2.4"/><path d="M50 40 C56 44 58 50 54 56" fill="none" stroke="#e9dcc0" stroke-width="1"/>')
I['isle_clothes'] = svg(
    lg('cl', (0, '#7aa8e0'), (.5, '#3f6eb0'), (1, '#223f70')) + lg('cb', (0, '#e8c46a'), (1, '#a07a2a')),
    shadow(32, 58, 22, 4) +
    f'<g {O}><path d="M22 8 L14 12 L4 24 L12 32 L17 27 L17 56 L47 56 L47 27 L52 32 L60 24 L50 12 L42 8 C40 13 36 15 32 15 C28 15 24 13 22 8 Z" fill="url(#cl)"/></g>'
    '<path d="M32 16 L32 55" stroke="#1a2f54" stroke-width="1.6"/>'
    f'<g {O} stroke-width="1.6" fill="url(#cb)"><circle cx="35.5" cy="24" r="2.2"/><circle cx="35.5" cy="33" r="2.2"/><circle cx="35.5" cy="42" r="2.2"/></g>'
    '<path d="M20 30 L20 52 M44 30 L44 52" stroke="#9cc4f0" stroke-width="1.4" opacity=".6"/>'
    '<path d="M8 23 L15 14 L22 10" fill="none" stroke="#b9d6f6" stroke-width="1.6" opacity=".85"/>')
I['isle_grain'] = svg(
    lg('gr', (0, '#fbe08a'), (.5, '#e0b040'), (1, '#9a6a14')),
    shadow(32, 58, 18, 4) +
    '<g stroke="#2a1708" stroke-width="2.4" fill="none" stroke-linecap="round"><path d="M32 58 L32 18"/><path d="M32 56 L22 22"/><path d="M32 56 L42 22"/></g>'
    '<g stroke="#c9a042" stroke-width="1.2" fill="none"><path d="M32 58 L32 18"/><path d="M32 56 L22 22"/><path d="M32 56 L42 22"/></g>' +
    ''.join(f'<g {O} stroke-width="1.8" fill="url(#gr)"><ellipse cx="{x-3.2}" cy="{y}" rx="3" ry="5.2" transform="rotate(-22 {x-3.2} {y})"/><ellipse cx="{x+3.2}" cy="{y}" rx="3" ry="5.2" transform="rotate(22 {x+3.2} {y})"/></g>'
            for x, ys in ((32, (9, 18, 27)), (21, (14, 23)), (43, (14, 23))) for y in ys) +
    '<g fill="#2a1708"><ellipse cx="32" cy="5" rx="2.4" ry="3.6"/></g><path d="M32 2 L32 -3" stroke="#2a1708"/>')
I['isle_flour'] = svg(
    lg('sk', (0, '#f4ead4'), (.6, '#d9caa6'), (1, '#a8946c')),
    shadow(32, 58, 22, 4) +
    f'<g {O}><path d="M16 22 C12 34 10 46 14 56 L50 56 C54 46 52 34 48 22 Z" fill="url(#sk)"/><path d="M18 22 C22 18 26 14 32 14 C38 14 42 18 46 22 Z" fill="#fbf6ea"/><path d="M22 17 L28 9 L36 9 L42 17" fill="none"/></g>'
    '<path d="M24 12 L40 12" stroke="#8a6a3a" stroke-width="3"/>'
    '<path d="M24 32 C28 30 36 30 40 32 M26 40 C30 39 34 39 38 40" stroke="#8a7650" stroke-width="1.6" fill="none"/>'
    '<g fill="#3d6aa8"><path d="M27 44 L37 44 L32 51 Z"/></g>'
    '<path d="M18 30 C17 38 17 46 19 52" stroke="#fff" stroke-width="2" fill="none" opacity=".7"/>')
I['isle_hops'] = svg(
    lg('hp', (0, '#cfe88a'), (.5, '#8fbf3a'), (1, '#4a7a1a')) + lg('lf', (0, '#6aa83a'), (1, '#2f5a14')),
    shadow(32, 58, 18, 4) +
    f'<g {O}><path d="M30 6 C40 2 54 8 52 18 C46 14 38 14 30 6 Z" fill="url(#lf)"/></g>'
    '<path d="M30 6 C30 14 32 18 32 24" stroke="#2a1708" stroke-width="2.4" fill="none"/>' +
    ''.join(f'<g {O} stroke-width="2"><path d="M{x} {y-11} C{x+9} {y-8} {x+10} {y+6} {x} {y+12} C{x-10} {y+6} {x-9} {y-8} {x} {y-11} Z" fill="url(#hp)"/></g>'
            f'<path d="M{x-6} {y-2} C{x-2} {y+1} {x+2} {y+1} {x+6} {y-2} M{x-5} {y+5} C{x-2} {y+8} {x+2} {y+8} {x+5} {y+5}" stroke="#4a7a1a" stroke-width="1.3" fill="none"/>'
            f'<path d="M{x-4} {y-8} C{x-6} {y-4} {x-6} {y} {x-5} {y+3}" stroke="#eaf8c0" stroke-width="1.4" fill="none" opacity=".8"/>'
            for x, y in ((22, 34), (42, 34), (32, 44))))
I['isle_ale'] = svg(
    lg('gl', (0, '#e9a83a', ), (.6, '#c0741a'), (1, '#7a3e0a'), x1=0, y1=0, x2=1, y2=0) + lg('fo', (0, '#ffffff'), (1, '#e8dcc0')),
    shadow(30, 58, 20, 4) +
    f'<g {O}><path d="M44 26 C56 26 56 46 44 46" fill="none" stroke-width="7"/></g>'
    '<path d="M44 26 C56 26 56 46 44 46" fill="none" stroke="#c9a46a" stroke-width="3.2"/>'
    f'<g {O}><path d="M14 18 L46 18 L44 56 L16 56 Z" fill="url(#gl)"/>'
    '<path d="M11 18 C10 10 16 8 20 10 C22 5 30 5 32 9 C35 5 44 6 44 12 C50 12 50 20 46 20 L13 20 C10 20 10 18 11 18 Z" fill="url(#fo)"/></g>'
    '<path d="M20 24 L21 52" stroke="#ffd98a" stroke-width="2.6" opacity=".8"/><path d="M38 24 L37 52" stroke="#6a3008" stroke-width="1.6" opacity=".6"/>'
    '<g fill="#ffe9b0" opacity=".9"><circle cx="28" cy="34" r="1.3"/><circle cx="32" cy="42" r="1"/><circle cx="26" cy="46" r="1.2"/></g>')
I['isle_clay'] = svg(
    lg('cy', (0, '#e09a6a'), (.5, '#b8653a'), (1, '#7a3a1a')),
    shadow(32, 56, 26, 5) +
    f'<g {O}><path d="M6 50 C4 38 12 30 22 32 C24 22 38 20 42 28 C52 24 62 34 58 44 C60 52 52 56 44 54 L14 56 C8 56 6 54 6 50 Z" fill="url(#cy)"/></g>'
    '<path d="M14 40 C18 36 24 36 28 38 M34 34 C38 31 44 32 47 35" stroke="#f0b48a" stroke-width="1.8" fill="none" opacity=".85"/>'
    '<path d="M22 48 C28 50 38 50 46 47" stroke="#6a2e12" stroke-width="1.4" fill="none"/>'
    f'<g {O} stroke-width="2"><path d="M40 8 L52 4 L56 20 L50 22 L44 10 Z" fill="#8a8f96"/><path d="M50 22 L46 34" stroke-width="3.4"/></g><path d="M50 22 L46 34" stroke="#a87444" stroke-width="1.6"/>')
I['isle_bricks'] = svg(
    lg('bf', (0, '#e07a52'), (.5, '#b84a2a'), (1, '#7a2a12')) + lg('bt', (0, '#f2a07a'), (1, '#c86a44')),
    shadow(32, 57, 27, 5) +
    ''.join(f'<g {O} stroke-width="2.2"><path d="M{x} {y} L{x+22} {y} L{x+22} {y+10} L{x} {y+10} Z" fill="url(#bf)"/><path d="M{x} {y} L{x+6} {y-5} L{x+28} {y-5} L{x+22} {y} Z" fill="url(#bt)"/><path d="M{x+22} {y} L{x+28} {y-5} L{x+28} {y+5} L{x+22} {y+10} Z" fill="#6a2410"/></g>'
            for x, y in ((6, 44), (30, 44), (18, 31), (12, 18) if False else (18, 31))) +
    f'<g {O} stroke-width="2.2"><path d="M18 31 L40 31 L40 41 L18 41 Z" fill="url(#bf)"/><path d="M18 31 L24 26 L46 26 L40 31 Z" fill="url(#bt)"/><path d="M40 31 L46 26 L46 36 L40 41 Z" fill="#6a2410"/></g>'
    '<path d="M9 47 L26 47 M33 47 L50 47 M21 34 L38 34" stroke="#f6b896" stroke-width="1.2" opacity=".8"/>')
I['isle_house'] = svg(
    lg('wa', (0, '#e88a5a'), (.6, '#c05a32'), (1, '#8a3418')) + lg('rf', (0, '#6a7c90'), (.5, '#4a5a6c'), (1, '#2a3440')) + lg('sd', (0, '#a8442a'), (1, '#6a2410')),
    shadow(32, 58, 27, 4) +
    f'<g {O}><path d="M10 30 L10 56 L40 56 L40 30 Z" fill="url(#wa)"/><path d="M40 30 L54 22 L54 50 L40 56 Z" fill="url(#sd)"/>'
    '<path d="M6 32 L22 12 L44 30 Z" fill="url(#rf)"/><path d="M22 12 L50 6 L58 22 L44 30 Z" fill="url(#rf)"/>'
    '<path d="M44 8 L44 2 L50 1 L50 7" fill="#8a3418"/></g>'
    f'<g {O} stroke-width="1.8"><rect x="15" y="36" width="8" height="8" fill="#ffd27a"/><rect x="28" y="40" width="8" height="16" fill="#5a2a14"/></g>'
    '<path d="M19 36 L19 44 M15 40 L23 40" stroke="#2a1708" stroke-width="1.2"/>'
    '<path d="M10 30 L24 13" stroke="#a8b8c8" stroke-width="1.6"/><path d="M12 54 L12 33" stroke="#f6b08a" stroke-width="1.4" opacity=".7"/>')
I['isle_workshop'] = svg(
    lg('ww', (0, '#d8b48a'), (.6, '#a88052'), (1, '#6a4a28')) + lg('wr', (0, '#8a5a3a'), (1, '#4a2a14')) + lg('cm', (0, '#c8c0b0'), (1, '#6a645a')),
    shadow(32, 58, 28, 4) +
    f'<g {O}><path d="M44 30 L44 8 L52 8 L52 30" fill="url(#cm)"/>'
    '<path d="M6 32 L6 56 L58 56 L58 32 Z" fill="url(#ww)"/><path d="M4 34 L18 20 L32 34 L46 20 L60 34 Z" fill="url(#wr)"/></g>'
    f'<g {O} stroke-width="1.8"><rect x="12" y="40" width="10" height="16" fill="#4a2a14"/><rect x="28" y="40" width="10" height="8" fill="#ffd27a"/><rect x="44" y="40" width="10" height="8" fill="#ffd27a"/></g>'
    '<g fill="#ffffff" opacity=".55"><circle cx="49" cy="3" r="3"/><circle cx="55" cy="-1" r="2.4"/></g>'
    '<path d="M6 34 L18 22" stroke="#c8946a" stroke-width="1.6"/>')
I['isle_farm'] = svg(
    lg('fd', (0, '#f6dc7a'), (.6, '#d4a83a'), (1, '#8a6414'), x1=0, y1=0, x2=0, y2=1) + lg('bn', (0, '#d0583a'), (.6, '#a03420'), (1, '#6a1e10')),
    shadow(32, 60, 28, 3) +
    f'<g {O}><path d="M2 50 L62 50 L62 60 L2 60 Z" fill="url(#fd)"/>'
    '<path d="M8 50 L8 28 L22 16 L36 28 L36 50 Z" fill="url(#bn)"/><path d="M4 30 L22 13 L40 30" fill="none" stroke-width="5"/></g>'
    '<path d="M4 30 L22 13 L40 30" fill="none" stroke="#5a5a5a" stroke-width="2.4"/>'
    f'<g {O} stroke-width="1.8"><path d="M16 50 L16 38 L28 38 L28 50" fill="#5a2010"/></g><path d="M16 38 L28 50 M28 38 L16 50" stroke="#f0d0b0" stroke-width="1.5"/>'
    '<g stroke="#2a1708" stroke-width="2.2" fill="none"><path d="M46 50 L46 32 M42 36 L46 32 L50 36 M54 50 L54 38 M51 41 L54 38 L57 41"/></g>'
    '<g stroke="#e8c45a" stroke-width="1" fill="none"><path d="M46 50 L46 32 M54 50 L54 38"/></g>'
    '<path d="M2 54 L62 54" stroke="#b8862a" stroke-width="1.2" opacity=".7"/>')
I['isle_chapel'] = svg(
    lg('st', (0, '#f2ece0'), (.6, '#cfc6b2'), (1, '#8e8470')) + lg('sr', (0, '#6aa0a8'), (.6, '#3e7078'), (1, '#1e3c42')),
    shadow(32, 60, 26, 3) +
    f'<g {O}><path d="M24 30 L24 58 L40 58 L40 30" fill="url(#st)"/><path d="M22 32 L32 6 L42 32 Z" fill="url(#sr)"/>'
    '<path d="M8 40 L8 58 L24 58 L24 40 L16 32 Z" fill="url(#st)"/><path d="M40 40 L40 58 L56 58 L56 40 L48 32 Z" fill="url(#st)"/>'
    '<path d="M29 58 L29 46 C29 42 35 42 35 46 L35 58" fill="#4a3020"/></g>'
    '<path d="M32 0 L32 8 M28.5 3.5 L35.5 3.5" stroke="#2a1708" stroke-width="4"/><path d="M32 0 L32 8 M28.5 3.5 L35.5 3.5" stroke="#f0c85a" stroke-width="2"/>'
    f'<g {O} stroke-width="1.5"><circle cx="32" cy="37" r="3" fill="#ffd27a"/><path d="M14 48 L14 44 C14 42 18 42 18 44 L18 48 Z M46 48 L46 44 C46 42 50 42 50 44 L50 48 Z" fill="#ffd27a"/></g>'
    '<path d="M24 31 L32 8" stroke="#a8dce0" stroke-width="1.4" opacity=".8"/>')
I['isle_anchor'] = svg(
    lg('an', (0, '#d8e4ee'), (.5, '#8aa4bc'), (1, '#3c5670')),
    shadow(32, 60, 22, 3) +
    '<g stroke="#2a1708" stroke-width="7.5" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="32" cy="10" r="5"/><path d="M32 15 L32 56"/><path d="M21 24 L43 24"/><path d="M9 38 C9 50 21 56 32 56 C43 56 55 50 55 38"/><path d="M5 42 L9 35 L15 40 M59 42 L55 35 L49 40"/></g>'
    '<g stroke="url(#an)" stroke-width="3.8" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="32" cy="10" r="5"/><path d="M32 15 L32 56"/><path d="M21 24 L43 24"/><path d="M9 38 C9 50 21 56 32 56 C43 56 55 50 55 38"/><path d="M5 42 L9 35 L15 40 M59 42 L55 35 L49 40"/></g>'
    '<path d="M30.6 18 L30.6 50 M12 40 C13 46 18 50 24 52" stroke="#ffffff" stroke-width="1.1" fill="none" opacity=".8"/>'
    '<path d="M32 28 C40 30 44 36 40 42 C36 48 26 46 26 40" fill="none" stroke="#2a1708" stroke-width="4"/><path d="M32 28 C40 30 44 36 40 42 C36 48 26 46 26 40" fill="none" stroke="#c8a46a" stroke-width="2"/>')
I['isle_airship'] = svg(
    lg('en', (0, '#fbf4e2'), (.5, '#ddd0b0'), (1, '#9a8a6a')) + lg('fi', (0, '#e0583a'), (1, '#8a2414')),
    shadow(32, 61, 18, 2.5) +
    f'<g {O}><path d="M50 15 L61 7 L61 23 Z M50 37 L61 45 L61 29 Z" fill="url(#fi)"/>'
    '<path d="M3 26 C3 14 25 9 40 11 C52 13 58 19 58 26 C58 33 52 39 40 41 C25 43 3 38 3 26 Z" fill="url(#en)"/>'
    '<path d="M20 47 L40 47 L38 55 L22 55 Z" fill="#7a5030"/></g>'
    '<path d="M24 41 L22 47 M36 41 L38 47" stroke="#2a1708" stroke-width="1.8"/>'
    '<path d="M14 15 C14 26 14 30 15 37 M28 11 C27 22 27 30 28 41 M42 12 C43 22 43 30 42 40" stroke="#a8987a" stroke-width="1.2" fill="none"/>'
    '<path d="M10 20 C18 14 32 13 44 15" fill="none" stroke="#ffffff" stroke-width="2.2" opacity=".9"/>'
    '<g fill="#ffd27a" stroke="#2a1708" stroke-width="1"><rect x="25" y="49" width="3" height="3"/><rect x="31" y="49" width="3" height="3"/></g>')
I['isle_road'] = svg(
    lg('rd', (0, '#e0cc9e'), (.6, '#b49a6a'), (1, '#7a6440')) + rg('tr', (0, '#8ac85a'), (1, '#3a6a1a')),
    shadow(32, 60, 22, 3) +
    f'<g {O}><path d="M20 60 L28 4 L36 4 L44 60 Z" fill="url(#rd)"/></g>'
    '<g fill="#8a7450" opacity=".8"><circle cx="27" cy="44" r="1.6"/><circle cx="37" cy="30" r="1.4"/><circle cx="33" cy="52" r="1.8"/><circle cx="31" cy="18" r="1.2"/></g>'
    '<path d="M24 56 L30 8" stroke="#f6ead0" stroke-width="1.4" opacity=".8"/>'
    f'<g {O}><circle cx="11" cy="22" r="7" fill="url(#tr)"/><circle cx="53" cy="42" r="7" fill="url(#tr)"/></g>'
    '<path d="M11 29 L11 34 M53 49 L53 54" stroke="#2a1708" stroke-width="2.6"/>')
I['isle_people'] = svg(
    rg('pa', (0, '#fde0c0'), (.7, '#e0a878'), (1, '#a8704a')) + lg('pb', (0, '#7aa8e0'), (1, '#2c4f86')) + lg('pc', (0, '#e0684a'), (1, '#8a2a1a')),
    shadow(32, 58, 24, 4) +
    f'<g {O}><path d="M30 56 C30 40 35 33 43 33 C51 33 56 40 56 56 Z" fill="url(#pc)"/><circle cx="43" cy="22" r="8" fill="url(#pa)"/>'
    '<path d="M35 18 C36 12 50 12 51 18 L35 18 Z" fill="#4a3020"/>'
    '<path d="M6 56 C6 38 12 31 22 31 C32 31 38 38 38 56 Z" fill="url(#pb)"/><circle cx="22" cy="19" r="9" fill="url(#pa)"/>'
    '<path d="M12 16 C12 6 32 6 32 16 C28 13 16 13 12 16 Z" fill="#2a3a5a"/></g>'
    '<path d="M11 50 L11 40 C11 36 14 34 17 33" stroke="#b9d6f6" stroke-width="1.6" fill="none" opacity=".8"/>')
I['isle_demolish'] = svg(
    lg('hd', (0, '#e4eaf0'), (.5, '#9aa4b0'), (1, '#4a5460')) + lg('hw', (0, '#c08a52'), (1, '#6a4420')),
    shadow(32, 58, 22, 4) +
    f'<g {O}><path d="M31 33 L11 55 L4 49 L25 27 Z" fill="url(#hw)"/>'
    '<path d="M19 21 L35 5 C44 7 53 14 59 24 L51 31 C47 25 42 21 36 19 L28 29 Z" fill="url(#hd)"/></g>'
    '<path d="M24 22 L36 9 C42 10 48 14 52 19" stroke="#ffffff" stroke-width="1.6" fill="none" opacity=".85"/>'
    f'<g {O} stroke-width="1.8" fill="#b84a2a"><path d="M40 48 L48 44 L54 52 L46 56 Z"/><path d="M50 40 L56 38 L60 44 L54 46 Z"/></g>'
    '<g fill="#c8b48a"><circle cx="36" cy="54" r="1.6"/><circle cx="58" cy="54" r="1.3"/></g>')
I['isle_market'] = svg(
    lg('aw', (0, '#fbf6ea'), (1, '#d7c9aa')) + lg('ct', (0, '#c8965e'), (1, '#6e4a26')),
    shadow(32, 60, 28, 3) +
    f'<g {O}><path d="M10 34 L10 57 L54 57 L54 34" fill="url(#ct)"/><path d="M4 34 L12 12 L52 12 L60 34 Z" fill="url(#aw)"/></g>'
    '<path d="M12 12 L8 34 M25.3 12 L22.6 34 M38.6 12 L41.3 34 M52 12 L56 34" stroke="#c0472e" stroke-width="5"/>'
    '<path d="M4 34 L12 12 L52 12 L60 34 Z" fill="none" stroke="#2a1708" stroke-width="2.6" stroke-linejoin="round"/>'
    '<path d="M4 34 C8 38 12 38 16 34 C20 38 24 38 28 34 C32 38 36 38 40 34 C44 38 48 38 52 34 C56 38 60 38 60 34" fill="#f6efe0" stroke="#2a1708" stroke-width="2"/>'
    f'<g {O} stroke-width="1.8"><circle cx="22" cy="47" r="5" fill="#e0a43a"/><circle cx="33" cy="48" r="5" fill="#8fbf4a"/><circle cx="43" cy="47" r="5" fill="#d0532e"/></g>'
    '<g fill="#fff" opacity=".7"><circle cx="20.5" cy="45.5" r="1.3"/><circle cx="31.5" cy="46.5" r="1.3"/><circle cx="41.5" cy="45.5" r="1.3"/></g>')
out = sys.argv[1]
os.makedirs(out, exist_ok=True)
import re
for n, s in I.items():
    s = re.sub(r'stroke="#2a1708" stroke-width="2.6" (stroke-linejoin="round" stroke-linecap="round") stroke-width="([\d.]+)"', r'stroke="#2a1708" stroke-width="\2" \1', s)
    open(os.path.join(out, n + '.svg'), 'w').write(s)
print(len(I))
