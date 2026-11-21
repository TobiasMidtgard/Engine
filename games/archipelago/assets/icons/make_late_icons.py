# Painted-style icons for Brightsund's fourth and fifth tiers, in the hand of make_icons.py:
# goods isle_ore, isle_iron, isle_coffee_beans, isle_coffee, isle_clocks, isle_cacao,
# isle_chocolate, isle_silk, isle_gowns, isle_gold, isle_jewellery; the tier hats
# isle_tier_masters (a ship's master's peaked cap) and isle_tier_patricians (a coronet); and the
# public buildings isle_theatre (two masks) and isle_bank (a pillared front).
# Usage: python3 make_late_icons.py <out dir>
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
def hl(d, w=1.8, op=.7):
    return f'<path d="{d}" fill="none" stroke="#fff" stroke-width="{w}" opacity="{op}" stroke-linecap="round"/>'
GOLD = lg('gd', (0, '#fff2a0'), (.5, '#e8b030'), (1, '#8a5a10'))
WOOD = lg('wd', (0, '#d8a46a'), (.55, '#9a6432'), (1, '#5a3414'))
I = {}

# Ore: grey rock lumps with rusty veins.
I['isle_ore'] = svg(rg('rk', (0, '#b8b4ac'), (.6, '#7a756c'), (1, '#45413a')) + rg('rs', (0, '#e88a50'), (1, '#8a3a14')),
    shadow(32, 55, 26, 5) +
    f'<g {O}><path d="M8 50 L12 36 L24 30 L32 38 L30 52 Z" fill="url(#rk)"/>'
    '<path d="M28 52 L30 32 L42 22 L56 30 L58 48 L46 56 Z" fill="url(#rk)"/>'
    '<path d="M18 30 L22 18 L34 14 L40 24 L30 34 L22 32 Z" fill="url(#rk)"/></g>'
    '<g fill="url(#rs)" stroke="#2a1708" stroke-width="1.2"><path d="M36 34 L44 30 L48 36 L40 40 Z"/><path d="M14 42 L20 40 L22 46 L16 47 Z"/><path d="M26 22 L31 20 L32 25 Z"/><path d="M46 44 L52 42 L52 48 Z"/></g>'
    + hl('M24 18 L33 15') + hl('M33 33 L42 24'))

# Iron: two dark bars stacked, cut faces lit.
I['isle_iron'] = svg(lg('ib', (0, '#9aa8b8'), (.5, '#5a6676'), (1, '#2a3038')) + lg('it', (0, '#d8e2ec'), (1, '#8a96a4')),
    shadow(32, 56, 26, 5) +
    ''.join(f'<g {O}><path d="M8 {y} L40 {y} L46 {y-8} L14 {y-8} Z" fill="url(#it)"/><path d="M8 {y} L40 {y} L40 {y+10} L8 {y+10} Z" fill="url(#ib)"/><path d="M40 {y} L46 {y-8} L46 {y+2} L40 {y+10} Z" fill="url(#ib)"/></g>' for y in (44,))
    + ''.join(f'<g {O}><path d="M18 {y} L50 {y} L56 {y-8} L24 {y-8} Z" fill="url(#it)"/><path d="M18 {y} L50 {y} L50 {y+10} L18 {y+10} Z" fill="url(#ib)"/><path d="M50 {y} L56 {y-8} L56 {y+2} L50 {y+10} Z" fill="url(#ib)"/></g>' for y in (28,))
    + hl('M22 22 L50 22', 1.6) + hl('M12 38 L40 38', 1.6))

# Coffee beans: three brown beans with their crease.
def bean(cx, cy, rot):
    return (f'<g transform="rotate({rot} {cx} {cy})"><ellipse cx="{cx}" cy="{cy}" rx="10" ry="13" fill="url(#cb)" {O}/>'
            f'<path d="M{cx} {cy-11} C{cx-5} {cy-4} {cx+5} {cy+4} {cx} {cy+11}" fill="none" stroke="#2a1708" stroke-width="2"/>'
            f'<path d="M{cx-6} {cy-6} C{cx-5} {cy-9} {cx-3} {cy-10} {cx-1} {cy-11}" fill="none" stroke="#f0c090" stroke-width="1.6" opacity=".8"/></g>')
I['isle_coffee_beans'] = svg(rg('cb', (0, '#b07a48'), (.6, '#6a3a1a'), (1, '#3a1c08')),
    shadow(32, 56, 24, 5) + bean(20, 40, -30) + bean(44, 40, 25) + bean(32, 24, 5))

# Coffee: a cup on a saucer, with steam.
I['isle_coffee'] = svg(lg('cp', (0, '#ffffff'), (.6, '#e8e0d0'), (1, '#a89c88')) + lg('cf', (0, '#8a5028'), (1, '#3a1a08')),
    shadow(32, 57, 26, 4.5) +
    f'<g {O}><ellipse cx="32" cy="50" rx="26" ry="6" fill="url(#cp)"/>'
    '<path d="M44 34 C54 32 56 44 44 46" fill="none" stroke-width="5"/>'
    '<path d="M14 30 L50 30 C50 42 44 50 32 50 C20 50 14 42 14 30 Z" fill="url(#cp)"/>'
    '<ellipse cx="32" cy="30" rx="18" ry="4" fill="url(#cf)"/></g>'
    '<path d="M44 34 C52 33 53 43 44 45" fill="none" stroke="#e8e0d0" stroke-width="2"/>'
    '<g fill="none" stroke="#fff" stroke-width="2.2" opacity=".75" stroke-linecap="round"><path d="M26 22 C22 18 30 14 26 8"/><path d="M36 22 C32 18 40 14 36 8"/></g>'
    + hl('M18 34 C18 40 22 45 26 47', 2))

# Clocks: a mantel clock, a wooden case with a white face and gold rim.
I['isle_clocks'] = svg(WOOD + GOLD + rg('fc', (0, '#ffffff'), (1, '#e0d8c4')),
    shadow(32, 58, 22, 4) +
    f'<g {O}><path d="M12 56 L12 30 C12 14 52 14 52 30 L52 56 Z" fill="url(#wd)"/>'
    '<path d="M8 56 L56 56 L56 60 L8 60 Z" fill="url(#wd)"/>'
    '<circle cx="32" cy="34" r="14" fill="url(#gd)"/><circle cx="32" cy="34" r="11" fill="url(#fc)"/></g>'
    + ''.join(f'<rect x="31.2" y="24.5" width="1.6" height="3" fill="#2a1708" transform="rotate({a} 32 34)"/>' for a in range(0, 360, 30)) +
    '<path d="M32 34 L32 26 M32 34 L38 37" stroke="#2a1708" stroke-width="2.2" stroke-linecap="round"/><circle cx="32" cy="34" r="1.6" fill="#2a1708"/>'
    + hl('M18 26 C20 20 26 17 31 17', 2))

# Cacao: a ribbed yellow-orange pod with a leaf.
I['isle_cacao'] = svg(rg('cc', (0, '#ffd060'), (.6, '#e07a18'), (1, '#8a3a08')) + lg('lf', (0, '#8ad050'), (1, '#2a6a18')),
    shadow(32, 57, 22, 4.5) +
    f'<g {O}><path d="M44 10 C56 8 60 18 54 24 C50 18 46 14 44 10 Z" fill="url(#lf)"/>'
    '<path d="M32 12 C48 14 52 34 44 48 C38 58 26 58 20 48 C12 34 16 14 32 12 Z" fill="url(#cc)"/></g>'
    '<g fill="none" stroke="#8a3a08" stroke-width="1.6" opacity=".8"><path d="M32 13 C30 26 30 44 32 56"/><path d="M24 16 C20 28 20 42 24 52"/><path d="M40 16 C44 28 44 42 40 52"/></g>'
    '<path d="M32 12 L34 6" stroke="#5a3414" stroke-width="3" stroke-linecap="round"/>'
    + hl('M22 22 C24 18 27 16 30 15', 2.2))

# Chocolate: a bar of squares, half out of its gold-edged paper.
I['isle_chocolate'] = svg(lg('ch', (0, '#8a5030'), (.5, '#5a2c14'), (1, '#2a1006')) + lg('wr', (0, '#d84a3a'), (1, '#7a1a10')) + GOLD,
    shadow(32, 57, 26, 4.5) +
    f'<g {O}><path d="M10 22 L44 12 L54 40 L20 50 Z" fill="url(#ch)"/>'
    '<path d="M28 44 L54 36 L58 48 L32 56 Z" fill="url(#wr)"/></g>'
    '<path d="M28 44 L54 36" stroke="url(#gd)" stroke-width="3"/>'
    '<g fill="none" stroke="#2a1006" stroke-width="1.6"><path d="M21 19 L28 40"/><path d="M32 16 L39 37"/><path d="M14 33 L48 23"/></g>'
    '<g fill="none" stroke="#b07a50" stroke-width="1.2" opacity=".8"><path d="M13 23 L19 21 M24 20 L30 18 M35 17 L41 15 M17 34 L23 32 M28 31 L34 29"/></g>')

# Silk: a sheen-striped bolt of rose silk, unrolling.
I['isle_silk'] = svg(lg('sk', (0, '#ffc8e8'), (.5, '#d0508a'), (1, '#6a1a40'), x1=0, y1=0, x2=1, y2=0) + rg('se', (0, '#ffe0f0'), (1, '#a83a6a')),
    shadow(32, 57, 26, 4.5) +
    f'<g {O}><path d="M30 34 C40 40 52 38 60 50 L40 56 C34 48 26 46 20 44 Z" fill="url(#sk)"/>'
    '<path d="M10 14 L34 14 L34 42 L10 42 Z" fill="url(#sk)"/>'
    '<ellipse cx="10" cy="28" rx="5" ry="14" fill="url(#se)"/><ellipse cx="34" cy="28" rx="5" ry="14" fill="url(#se)"/></g>'
    '<ellipse cx="10" cy="28" rx="1.6" ry="4" fill="#2a1708"/>'
    + hl('M14 18 L32 18', 2.4, .85) + hl('M36 42 C44 44 50 44 56 49', 1.8))

# Gowns: a flowing teal gown on a dress form.
I['isle_gowns'] = svg(lg('gw', (0, '#7ae0d0'), (.5, '#1a8a8a'), (1, '#0a3a44')) + WOOD,
    shadow(32, 58, 22, 4) +
    f'<g {O}><path d="M32 4 L32 10" fill="none"/><circle cx="32" cy="6" r="3" fill="url(#wd)"/>'
    '<path d="M24 12 C28 10 36 10 40 12 L38 26 C46 36 52 46 54 56 L10 56 C12 46 18 36 26 26 Z" fill="url(#gw)"/></g>'
    '<path d="M25 25 C30 27 34 27 39 25" stroke="#e8c050" stroke-width="2.4" fill="none"/>'
    '<g fill="none" stroke="#0a3a44" stroke-width="1.6" opacity=".8"><path d="M30 28 C26 38 22 46 20 55"/><path d="M34 28 C38 38 42 46 46 55"/><path d="M32 28 L32 55"/></g>'
    + hl('M27 14 C26 18 26 22 27 25', 2) + hl('M24 32 C20 40 17 46 15 53', 1.8, .55))

# Gold: three nuggets with a glint.
I['isle_gold'] = svg(GOLD + rg('gn', (0, '#fff8c0'), (.5, '#f0b828'), (1, '#8a5208')),
    shadow(32, 56, 24, 5) +
    f'<g {O}><path d="M8 48 C6 40 14 34 22 36 C30 34 34 42 30 50 C24 56 12 56 8 48 Z" fill="url(#gn)"/>'
    '<path d="M30 46 C28 36 36 30 46 32 C56 32 60 42 54 50 C46 56 34 56 30 46 Z" fill="url(#gn)"/>'
    '<path d="M20 30 C18 20 26 14 34 16 C42 18 44 28 38 34 C32 38 22 38 20 30 Z" fill="url(#gn)"/></g>'
    '<g fill="#8a5208" opacity=".55"><circle cx="16" cy="46" r="1.6"/><circle cx="44" cy="42" r="2"/><circle cx="30" cy="26" r="1.6"/></g>'
    + hl('M24 22 C26 19 29 18 32 18', 2.2, .9) +
    '<path d="M50 14 L52 20 L58 22 L52 24 L50 30 L48 24 L42 22 L48 20 Z" fill="#fff" opacity=".9"/>')

# Jewellery: a gold chain with a ruby pendant.
I['isle_jewellery'] = svg(GOLD + rg('rb', (0, '#ffb0b8'), (.45, '#e0203a'), (1, '#6a0814')),
    shadow(32, 58, 18, 4) +
    '<path d="M8 10 C10 30 22 38 32 38 C42 38 54 30 56 10" fill="none" stroke="#2a1708" stroke-width="5" stroke-linecap="round"/>'
    '<path d="M8 10 C10 30 22 38 32 38 C42 38 54 30 56 10" fill="none" stroke="url(#gd)" stroke-width="2.6" stroke-dasharray="3 1.6" stroke-linecap="round"/>'
    f'<g {O}><circle cx="32" cy="40" r="4" fill="url(#gd)"/>'
    '<path d="M32 42 L44 50 L32 62 L20 50 Z" fill="url(#rb)"/></g>'
    '<path d="M32 42 L32 62 M20 50 L44 50" stroke="#6a0814" stroke-width="1" opacity=".6"/>'
    + hl('M25 49 L31 45', 2, .85))

# Masters: a ship's master's peaked cap, navy with a gold badge.
I['isle_tier_masters'] = svg(lg('nv', (0, '#4a5a8a'), (.5, '#1e2a4a'), (1, '#0a1020')) + lg('vz', (0, '#3a3a3a'), (1, '#0a0a0a')) + GOLD,
    shadow(32, 58, 26, 3.5) +
    f'<g {o(2.6)}><path d="M6 30 C6 18 20 14 32 14 C46 14 60 18 58 30 C56 36 48 38 32 38 C16 38 8 36 6 30 Z" fill="url(#nv)"/>'
    '<path d="M12 34 L52 34 L50 42 L14 42 Z" fill="url(#nv)"/>'
    '<path d="M14 42 C22 52 42 52 50 42 C42 46 22 46 14 42 Z" fill="url(#vz)"/></g>'
    '<path d="M14 38 L50 38" stroke="url(#gd)" stroke-width="2.6"/>'
    f'<g {o(1.6)}><path d="M32 20 L36 26 L32 32 L28 26 Z" fill="url(#gd)"/></g>'
    '<path d="M24 28 C26 26 28 26 28 26 M36 26 C38 26 40 28 40 28" stroke="#e8b030" stroke-width="2" fill="none" stroke-linecap="round"/>'
    + hl('M12 24 C16 19 24 17 30 17', 2.2, .45))

# Patricians: a gold coronet with red velvet and pearls.
I['isle_tier_patricians'] = svg(GOLD + lg('vv', (0, '#e85a5a'), (1, '#6a0a14')),
    shadow(32, 58, 26, 3.5) +
    f'<g {o(2.4)}><path d="M14 40 C14 22 50 22 50 40 Z" fill="url(#vv)"/>'
    '<path d="M8 40 L12 22 L22 32 L32 16 L42 32 L52 22 L56 40 Z" fill="url(#gd)"/>'
    '<path d="M8 40 L56 40 L56 52 L8 52 Z" fill="url(#gd)"/></g>'
    '<g fill="#fff8f0" stroke="#2a1708" stroke-width="1.4"><circle cx="12" cy="21" r="3"/><circle cx="32" cy="14" r="3.4"/><circle cx="52" cy="21" r="3"/></g>'
    '<g stroke="#2a1708" stroke-width="1.4"><circle cx="20" cy="46" r="3" fill="#2a8ad8"/><circle cx="32" cy="46" r="3.4" fill="#e0203a"/><circle cx="44" cy="46" r="3" fill="#2aa860"/></g>'
    + hl('M12 43 L52 43', 1.6, .6))

# Theatre: the comic and the tragic mask.
I['isle_theatre'] = svg(lg('mk', (0, '#fff6dc'), (1, '#d8c08a')) + lg('mt', (0, '#a8c0e8'), (1, '#3a5a8a')) + GOLD,
    shadow(32, 58, 26, 4) +
    f'<g {O}><path d="M30 14 C40 10 54 12 58 18 C60 32 54 46 44 50 C36 50 30 40 30 14 Z" fill="url(#mt)"/></g>'
    '<g fill="#2a1708"><path d="M38 24 C40 22 44 22 46 25 C44 26 40 26 38 24 Z"/><path d="M48 22 C50 20 54 21 55 24 C53 25 50 25 48 22 Z"/><path d="M40 40 C44 34 50 34 54 38 C50 38 44 38 40 40 Z"/></g>'
    f'<g {O}><path d="M6 18 C10 12 24 10 34 14 C34 38 28 50 20 50 C10 46 4 32 6 18 Z" fill="url(#mk)"/></g>'
    '<g fill="#2a1708"><path d="M11 24 C13 21 17 21 19 24 C17 25 13 25 11 24 Z"/><path d="M23 23 C25 20 29 20 31 23 C29 24 25 24 23 23 Z"/><path d="M12 34 C16 42 26 42 30 33 C24 36 18 36 12 34 Z"/></g>'
    '<path d="M20 50 C22 56 18 60 14 60" fill="none" stroke="url(#gd)" stroke-width="2.6"/>'
    + hl('M10 18 C14 15 20 14 26 14', 2, .8))

# Bank: a pillared front under a pediment, a gold coin over the door.
I['isle_bank'] = svg(lg('st', (0, '#f6f0e0'), (.6, '#d0c4a8'), (1, '#8a7c60')) + GOLD,
    shadow(32, 59, 28, 3.5) +
    f'<g {O}><path d="M4 22 L32 6 L60 22 Z" fill="url(#st)"/>'
    '<path d="M8 22 L56 22 L56 26 L8 26 Z" fill="url(#st)"/>'
    '<path d="M6 50 L58 50 L58 56 L6 56 Z" fill="url(#st)"/><path d="M2 56 L62 56 L62 60 L2 60 Z" fill="url(#st)"/></g>'
    + ''.join(f'<g {o(2)}><path d="M{x} 26 L{x+6} 26 L{x+6} 50 L{x} 50 Z" fill="url(#st)"/></g><path d="M{x+2} 28 L{x+2} 48" stroke="#a89878" stroke-width="1"/>' for x in (11, 21, 37, 47)) +
    f'<g {o(2)}><circle cx="32" cy="16" r="5" fill="url(#gd)"/></g>'
    '<circle cx="32" cy="16" r="2.6" fill="none" stroke="#8a5a10" stroke-width="1.4"/>'
    f'<g {o(1.8)}><path d="M29 50 L29 36 C29 32 35 32 35 36 L35 50 Z" fill="#3a2a18"/></g>'
    + hl('M12 20 L30 10', 1.8, .8))

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
for n, s in I.items():
    open(os.path.join(out, n + '.svg'), 'w').write(s)
print(len(I))
