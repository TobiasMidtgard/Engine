# Painted-style icons for the far regions' peoples and goods, in the hand of make_icons.py.
# Tier hats, one for each far people:
#   Sarrowmere: isle_tier_riverfolk (a woven cone hat), isle_tier_freeholders (a madras head
#   wrap), isle_tier_planters (a pith helmet);
#   the Amber Reach: isle_tier_herders (a headcloth under a cord), isle_tier_caravaners (an
#   indigo turban and veil), isle_tier_scholars (a tall starred cap with a tassel);
#   Frostholm: isle_tier_trappers (a fur hat with ear flaps), isle_tier_whalers (a sou'wester),
#   isle_tier_wardens (a wool cap with brass snow goggles).
# Goods: isle_plantains, isle_cotton, isle_calico, isle_goat_hair, isle_robes, isle_tea,
# isle_carpets.
# Usage: python3 make_far_icons.py <out dir>
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
BRASS = lg('br', (0, '#ffe8a0'), (.45, '#d8a038'), (1, '#6a4410'))
I = {}

# ---- Sarrowmere: the jungle ----

# Riverfolk: a broad woven cone hat, straw rings and a chin cord.
I['isle_tier_riverfolk'] = svg(lg('sw', (0, '#fff0b0'), (.5, '#e0b860'), (1, '#8a6420')),
    shadow(32, 54, 28, 4) +
    '<path d="M18 44 C20 54 44 54 46 44" fill="none" stroke="#6a3a14" stroke-width="2"/>'
    f'<g {O}><path d="M4 44 L32 10 L60 44 C50 50 14 50 4 44 Z" fill="url(#sw)"/></g>'
    + ''.join(f'<path d="M{32-k*5.6:.1f} {10+k*6.8:.1f} C{32-k*3:.1f} {13+k*7.4:.1f} {32+k*3:.1f} {13+k*7.4:.1f} {32+k*5.6:.1f} {10+k*6.8:.1f}" fill="none" stroke="#8a6420" stroke-width="1.4" opacity=".85"/>' for k in (1, 2, 3, 4)) +
    ''.join(f'<path d="M32 11 L{x} 46" stroke="#a87a30" stroke-width="1" opacity=".55"/>' for x in (12, 22, 32, 42, 52)) +
    f'<g {o(1.8)}><circle cx="32" cy="10" r="3" fill="#8a5a20"/></g>'
    + hl('M12 40 L30 15', 2, .55))

# Freeholders: a madras head wrap, red and gold check, knotted on top.
def check(id, a, b):
    return (f'<pattern id="{id}" width="8" height="8" patternUnits="userSpaceOnUse" patternTransform="rotate(-12)">'
            f'<rect width="8" height="8" fill="{a}"/><rect width="8" height="3" fill="{b}" opacity=".8"/>'
            f'<rect width="3" height="8" fill="{b}" opacity=".6"/><rect x="5" width="1" height="8" fill="#2a6ab0" opacity=".7"/></pattern>')
I['isle_tier_freeholders'] = svg(check('md', '#d83a2a', '#f4c040'),
    shadow(32, 56, 24, 4) +
    f'<g {O}><path d="M10 46 C6 30 14 20 32 20 C50 20 58 30 54 46 C46 50 18 50 10 46 Z" fill="url(#md)"/>'
    '<path d="M24 22 C20 12 28 4 34 10 C38 4 48 10 42 20 C38 24 28 24 24 22 Z" fill="url(#md)"/>'
    '<path d="M42 18 C50 14 56 18 54 26 C50 22 46 22 42 22 Z" fill="url(#md)"/></g>'
    '<g fill="none" stroke="#2a1708" stroke-width="1.6" opacity=".55"><path d="M12 38 C24 32 40 32 52 38"/><path d="M14 30 C24 26 40 26 50 30"/></g>'
    '<path d="M10 46 C18 50 46 50 54 46 L54 48 C46 53 18 53 10 48 Z" fill="#2a1708" opacity=".25"/>'
    + hl('M14 32 C18 25 24 22 30 22', 2.2, .55))

# Planters: a white pith helmet with a khaki band.
I['isle_tier_planters'] = svg(lg('ph', (0, '#ffffff'), (.6, '#ece4cc'), (1, '#a89c78')) + lg('kb', (0, '#b8a070'), (1, '#6a5430')),
    shadow(32, 54, 28, 4) +
    f'<g {O}><path d="M4 42 C4 36 60 36 60 42 C60 48 4 48 4 42 Z" fill="url(#ph)"/>'
    '<path d="M12 40 C10 18 22 10 32 10 C42 10 54 18 52 40 C42 44 22 44 12 40 Z" fill="url(#ph)"/>'
    '<path d="M12 34 C22 38 42 38 52 34 L52 40 C42 44 22 44 12 40 Z" fill="url(#kb)"/></g>'
    f'<g {o(1.6)}><circle cx="32" cy="10" r="2.6" fill="url(#kb)"/></g>'
    '<g fill="none" stroke="#a89c78" stroke-width="1.2" opacity=".8"><path d="M32 12 C28 18 26 26 26 34"/><path d="M32 12 C36 18 38 26 38 34"/></g>'
    + hl('M16 30 C16 20 22 14 28 13', 2.4, .9))

# ---- The Amber Reach: the desert ----

# Herders: a pale headcloth held by a doubled dark cord, falling to the shoulders.
I['isle_tier_herders'] = svg(lg('hc', (0, '#fffaf0'), (.6, '#e8dcc0'), (1, '#a8946c')) + lg('ag', (0, '#4a3a30'), (1, '#140c08')),
    shadow(32, 58, 26, 4) +
    f'<g {O}><path d="M8 58 C6 40 10 18 32 16 C54 18 58 40 56 58 C50 54 46 46 44 36 L20 36 C18 46 14 54 8 58 Z" fill="url(#hc)"/>'
    '<path d="M20 36 C22 46 42 46 44 36 Z" fill="#5a3a20"/></g>'
    '<g fill="none" stroke="#c8a878" stroke-width="1.4" opacity=".9"><path d="M14 52 C14 40 18 32 22 28"/><path d="M50 52 C50 40 46 32 42 28"/></g>'
    '<g fill="none" stroke="#c83a2a" stroke-width="1" opacity=".5"><path d="M10 50 L18 48 M12 44 L19 42 M54 50 L46 48 M52 44 L45 42"/></g>'
    f'<g {o(1.6)}><path d="M10 28 C18 22 46 22 54 28 C54 31 10 31 10 28 Z" fill="url(#ag)"/>'
    '<path d="M11 33 C18 27 46 27 53 33 C53 36 11 36 11 33 Z" fill="url(#ag)"/></g>'
    + hl('M16 22 C20 19 26 18 31 18', 2.2, .8))

# Caravaners: an indigo turban, wound in bands, its veil falling to one side.
I['isle_tier_caravaners'] = svg(lg('ig', (0, '#6a7ad8'), (.55, '#2a3488'), (1, '#0c1038')) + lg('ig2', (0, '#4a58b8'), (1, '#141a58')) + GOLD,
    shadow(32, 58, 26, 4) +
    f'<g {O}><path d="M40 36 C50 40 54 50 50 60 C44 58 40 50 36 42 Z" fill="url(#ig2)"/>'
    '<path d="M10 40 C6 26 16 12 32 12 C48 12 58 26 54 40 C44 44 20 44 10 40 Z" fill="url(#ig)"/></g>'
    '<g fill="none" stroke="#0c1038" stroke-width="1.8" opacity=".75"><path d="M10 34 C22 24 44 22 54 30"/><path d="M12 26 C24 18 40 16 50 22"/><path d="M14 40 C24 32 44 30 54 36"/></g>'
    '<g fill="none" stroke="#8a9af0" stroke-width="1.2" opacity=".7"><path d="M12 32 C22 23 42 21 52 28"/><path d="M14 24 C24 17 38 15 48 20"/></g>'
    f'<g {o(1.6)}><circle cx="32" cy="38" r="3" fill="url(#gd)"/></g>'
    + hl('M14 30 C16 22 22 16 28 15', 2.2, .5))

# Scholars: a tall wine-red cap with gold stars and a black tassel.
def star(cx, cy, r):
    import math
    pts = []
    for k in range(10):
        a = -math.pi / 2 + k * math.pi / 5
        rr = r if k % 2 == 0 else r * .45
        pts.append(f'{cx + rr * math.cos(a):.1f},{cy + rr * math.sin(a):.1f}')
    return f'<polygon points="{" ".join(pts)}" fill="url(#gd)" stroke="#5a3a08" stroke-width=".8"/>'
I['isle_tier_scholars'] = svg(lg('sc', (0, '#e85a5a'), (.5, '#9a1a2a'), (1, '#4a0810'), x2=1, y2=0) + GOLD,
    shadow(32, 58, 22, 4) +
    f'<g {O}><path d="M14 52 L18 12 C24 8 40 8 46 12 L50 52 C40 56 24 56 14 52 Z" fill="url(#sc)"/>'
    '<ellipse cx="32" cy="12" rx="14" ry="3.6" fill="#7a1420"/></g>'
    '<path d="M14 46 C24 50 40 50 50 46 L50 52 C40 56 24 56 14 52 Z" fill="url(#gd)" stroke="#2a1708" stroke-width="1.6"/>'
    + star(26, 24, 4.6) + star(39, 32, 3.8) + star(28, 38, 3) +
    '<path d="M32 12 C40 10 48 14 52 22" fill="none" stroke="#141010" stroke-width="2"/>'
    f'<g {o(1.4)}><path d="M50 22 L55 22 L57 36 L48 36 Z" fill="#241c18"/></g>'
    '<g stroke="#6a5a50" stroke-width=".8"><path d="M50 26 L50 35 M52.5 26 L52.5 35 M55 26 L55 35"/></g>'
    + hl('M19 18 L17 46', 2.2, .5))

# ---- Frostholm: the ice ----

# Trappers: a fur hat, ear flaps down, with a leather crown.
I['isle_tier_trappers'] = svg(rg('fr', (0, '#d8b890'), (.6, '#8a6440'), (1, '#4a3018')) + lg('lt', (0, '#a87048'), (1, '#4a2810')),
    shadow(32, 58, 26, 4) +
    f'<g {O}><path d="M8 32 C6 46 10 56 16 58 C20 52 20 42 20 34 Z" fill="url(#fr)"/>'
    '<path d="M56 32 C58 46 54 56 48 58 C44 52 44 42 44 34 Z" fill="url(#fr)"/>'
    '<path d="M14 34 C12 18 22 10 32 10 C42 10 52 18 50 34 C40 38 24 38 14 34 Z" fill="url(#lt)"/>'
    '<path d="M8 32 C8 24 56 24 56 32 C56 40 8 40 8 32 Z" fill="url(#fr)"/></g>'
    '<g fill="none" stroke="#4a3018" stroke-width="1.2" opacity=".7" stroke-linecap="round">'
    + ''.join(f'<path d="M{x} {28 + (x % 3)} l2 6"/>' for x in range(12, 54, 5)) +
    '<path d="M12 40 l1 6 M14 48 l1 5 M52 40 l-1 6 M50 48 l-1 5"/></g>'
    '<path d="M24 14 C28 22 28 28 32 30 C36 28 36 22 40 14" fill="none" stroke="#2a1708" stroke-width="1.2" opacity=".5"/>'
    + hl('M18 22 C20 16 26 13 30 12', 2, .6))

# Whalers: a yellow oilskin sou'wester, its brim long at the back.
I['isle_tier_whalers'] = svg(lg('oy', (0, '#fff27a'), (.5, '#f0c020'), (1, '#9a6a08')),
    shadow(32, 58, 28, 4) +
    f'<g {O}><path d="M4 40 C2 34 14 32 32 32 C50 32 62 34 60 40 C58 50 50 56 40 50 C34 46 30 46 24 50 C14 56 6 50 4 40 Z" fill="url(#oy)"/>'
    '<path d="M14 36 C12 18 22 10 32 10 C42 10 52 18 50 36 C40 40 24 40 14 36 Z" fill="url(#oy)"/></g>'
    '<g fill="none" stroke="#9a6a08" stroke-width="1.4" opacity=".85"><path d="M32 11 L32 38"/><path d="M22 14 C18 22 18 30 19 37"/><path d="M42 14 C46 22 46 30 45 37"/></g>'
    '<g fill="none" stroke="#2a1708" stroke-width="1" opacity=".45"><path d="M8 42 C18 38 46 38 56 42"/></g>'
    '<path d="M16 40 C14 50 18 58 22 60" fill="none" stroke="#5a3a14" stroke-width="1.8"/>'
    + hl('M18 32 C18 22 22 15 28 13', 2.4, .8))

# Wardens: a grey wool cap with a rolled brim and brass snow goggles strapped over it.
I['isle_tier_wardens'] = svg(lg('wl', (0, '#a8b4c0'), (.55, '#5a6878'), (1, '#262e38')) + BRASS + rg('gl', (0, '#e8f8ff'), (.5, '#6ab0d8'), (1, '#1a4a6a')),
    shadow(32, 58, 26, 4) +
    f'<g {O}><path d="M12 44 C8 22 20 10 32 10 C44 10 56 22 52 44 Z" fill="url(#wl)"/>'
    '<path d="M10 42 C20 38 44 38 54 42 L54 52 C44 48 20 48 10 52 Z" fill="url(#wl)"/></g>'
    '<g fill="none" stroke="#262e38" stroke-width="1.2" opacity=".6">'
    + ''.join(f'<path d="M{x} 41 L{x} 50"/>' for x in range(14, 52, 4)) + '</g>'
    '<path d="M11 30 C20 26 44 26 53 30" fill="none" stroke="#3a2210" stroke-width="5"/>'
    f'<g {o(2)}><circle cx="23" cy="29" r="7.5" fill="url(#br)"/><circle cx="41" cy="29" r="7.5" fill="url(#br)"/>'
    '<path d="M29 28 L35 28" fill="none" stroke-width="3"/></g>'
    f'<g {o(1.4)}><circle cx="23" cy="29" r="4.8" fill="url(#gl)"/><circle cx="41" cy="29" r="4.8" fill="url(#gl)"/></g>'
    '<circle cx="21.4" cy="27.4" r="1.4" fill="#fff" opacity=".9"/><circle cx="39.4" cy="27.4" r="1.4" fill="#fff" opacity=".9"/>'
    + hl('M18 18 C22 14 26 12 30 12', 2, .5))

# ---- Goods ----

# Plantains: a hand of three long green-gold fingers on a brown crown.
def finger(d):
    return f'<path d="{d}" fill="url(#pl)" {O}/>'
I['isle_plantains'] = svg(lg('pl', (0, '#e8f070'), (.5, '#9ac030'), (1, '#4a6a10'), x2=1, y2=1) + lg('pc', (0, '#8a5a28'), (1, '#3a2008')),
    shadow(32, 57, 26, 4.5) +
    finger('M12 18 C4 30 8 46 22 52 C26 52 26 48 22 46 C14 40 14 28 18 20 Z') +
    finger('M20 14 C16 32 24 48 40 52 C44 50 42 46 38 46 C28 40 26 28 26 16 Z') +
    finger('M28 12 C32 30 42 42 56 42 C60 38 56 36 52 36 C42 32 36 24 34 12 Z') +
    f'<g {O}><path d="M10 14 C16 6 30 6 36 10 L34 16 C28 12 18 12 14 18 Z" fill="url(#pc)"/></g>'
    '<g fill="#3a2008"><circle cx="22.4" cy="50" r="1.4"/><circle cx="40.6" cy="50" r="1.4"/><circle cx="56" cy="40" r="1.4"/></g>'
    + hl('M13 24 C10 32 12 40 18 46', 1.8, .6) + hl('M23 20 C22 30 26 38 32 44', 1.8, .6))

# Cotton: a boll of four white puffs in a brown husk, two leaves.
def puff(cx, cy, r):
    return f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="url(#ct)" {O}/>'
I['isle_cotton'] = svg(rg('ct', (0, '#ffffff'), (.65, '#f0ece4'), (1, '#b8b0a4')) + lg('hk', (0, '#a87040'), (1, '#4a2810')) + lg('cl', (0, '#8ad050'), (1, '#2a6a18')),
    shadow(32, 58, 24, 4) +
    f'<g {O}><path d="M30 56 C30 50 32 46 32 42" fill="none" stroke-width="3.4"/><path d="M32 48 C22 50 12 46 8 38 C18 36 26 40 32 48 Z" fill="url(#cl)"/>'
    '<path d="M33 50 C42 54 52 50 56 42 C46 40 38 44 33 50 Z" fill="url(#cl)"/>'
    '<path d="M16 40 L22 28 L32 42 L42 28 L48 40 C40 46 24 46 16 40 Z" fill="url(#hk)"/></g>'
    + puff(22, 26, 10) + puff(42, 26, 10) + puff(32, 16, 10) + puff(32, 32, 9) +
    hl('M17 22 C18 19 20 17 23 17', 2, .9) + hl('M28 11 C29 9 31 8 34 8', 2, .9))

# Calico: a folded bolt of cream cotton printed with small red flowers.
I['isle_calico'] = svg('<pattern id="cf" width="10" height="10" patternUnits="userSpaceOnUse"><rect width="10" height="10" fill="#f6ecd4"/>'
    '<circle cx="3" cy="3" r="1.8" fill="#d0302a"/><circle cx="8" cy="8" r="1.8" fill="#d0302a"/><circle cx="3" cy="3" r=".6" fill="#f4c040"/><circle cx="8" cy="8" r=".6" fill="#f4c040"/>'
    '<path d="M5 6 l1.4 -1.2 M0.5 8 l1.4 -1.2" stroke="#3a8a3a" stroke-width="1"/></pattern>',
    shadow(32, 57, 28, 4) +
    f'<g {O}><path d="M6 46 L14 22 L58 22 L50 46 Z" fill="url(#cf)"/>'
    '<path d="M6 46 L50 46 L50 54 L6 54 Z" fill="url(#cf)"/><path d="M50 46 L58 22 L58 30 L50 54 Z" fill="url(#cf)"/>'
    '<path d="M10 34 L54 34" fill="none" stroke-width="1.6"/></g>'
    '<path d="M50 46 L58 22 L58 30 L50 54 Z" fill="#000" opacity=".18"/><path d="M6 46 L50 46 L50 54 L6 54 Z" fill="#000" opacity=".08"/>'
    + hl('M15 24 L56 24', 1.6, .8))

# Goat hair: a hank of long grey-white hair, tied at the waist, its ends fanned out.
I['isle_goat_hair'] = svg(lg('gh', (0, '#fbf8f2'), (.55, '#cec6ba'), (1, '#7a746a')) + lg('tw', (0, '#d8a050'), (1, '#7a4a14')),
    shadow(32, 56, 28, 4.5) +
    f'<g {O}><path d="M30 26 C22 18 12 14 6 16 C2 22 2 30 4 36 C2 42 4 48 8 52 C14 52 22 46 30 40 Z" fill="url(#gh)"/>'
    '<path d="M34 26 C42 18 52 14 58 16 C62 22 62 30 60 36 C62 42 60 48 56 52 C50 52 42 46 34 40 Z" fill="url(#gh)"/></g>'
    '<g fill="none" stroke="#8a8278" stroke-width="1.2" opacity=".9" stroke-linecap="round">'
    '<path d="M30 30 C22 24 14 20 8 20"/><path d="M30 33 C20 30 12 28 5 30"/><path d="M30 36 C20 36 12 38 6 42"/><path d="M30 38 C22 42 16 46 10 50"/>'
    '<path d="M34 30 C42 24 50 20 56 20"/><path d="M34 33 C44 30 52 28 59 30"/><path d="M34 36 C44 36 52 38 58 42"/><path d="M34 38 C42 42 48 46 54 50"/></g>'
    '<g fill="none" stroke="#fff" stroke-width="1.2" opacity=".8" stroke-linecap="round"><path d="M28 28 C20 22 14 19 9 18"/><path d="M36 28 C44 22 50 19 55 18"/></g>'
    '<g fill="none" stroke="#2a1708" stroke-width="1.6" stroke-linecap="round"><path d="M6 16 l-3 -2 M4 36 l-3 1 M8 52 l-3 3 M58 16 l3 -2 M60 36 l3 1 M56 52 l3 3"/></g>'
    f'<g {o(1.8)}><path d="M28 24 C30 22 34 22 36 24 L36 42 C34 44 30 44 28 42 Z" fill="url(#tw)"/></g>'
    '<path d="M28 30 L36 30 M28 36 L36 36" stroke="#7a4a14" stroke-width="1.2"/>'
    '<path d="M32 43 C31 48 28 52 24 54 M32 43 C34 48 37 51 41 52" fill="none" stroke="#7a4a14" stroke-width="2" stroke-linecap="round"/>')

# Robes: a long striped kaftan with wide sleeves and a gold-edged opening.
I['isle_robes'] = svg('<pattern id="rs" width="8" height="64" patternUnits="userSpaceOnUse"><rect width="8" height="64" fill="#2a8a7a"/><rect width="3" height="64" fill="#e8d8a0"/><rect x="3" width="1" height="64" fill="#a83a2a"/></pattern>' + GOLD,
    shadow(32, 60, 22, 3.5) +
    f'<g {O}><path d="M24 8 L40 8 L58 22 L52 32 L44 26 L48 58 L16 58 L20 26 L12 32 L6 22 Z" fill="url(#rs)"/></g>'
    '<path d="M24 8 L40 8 L58 22 L52 32 L44 26 L48 58 L16 58 L20 26 L12 32 L6 22 Z" fill="#000" opacity=".12"/>'
    '<path d="M44 26 L48 58 L36 58 Z" fill="#000" opacity=".15"/>'
    '<path d="M27 8 C28 16 30 20 32 22 C34 20 36 16 37 8" fill="#3a2218" stroke="#2a1708" stroke-width="1.6"/>'
    '<path d="M27 8 C28 16 30 20 32 22 C34 20 36 16 37 8 M32 22 L32 58" fill="none" stroke="url(#gd)" stroke-width="2.4"/>'
    '<path d="M6 22 L12 32 M58 22 L52 32" stroke="url(#gd)" stroke-width="2.4"/>'
    + hl('M22 12 L10 21', 1.8, .7))

# Tea: a brass pot with a long curved spout and a domed lid, a green leaf beside it.
I['isle_tea'] = svg(BRASS + lg('tl', (0, '#9ae060'), (1, '#2a6a18')),
    shadow(32, 58, 26, 4) +
    f'<g {O}><path d="M44 36 C52 32 54 24 60 20 C58 26 56 36 46 46 Z" fill="url(#br)"/>'
    '<path d="M18 30 C10 30 8 44 18 46" fill="none" stroke-width="5"/>'
    '<path d="M20 26 C14 34 14 48 24 54 L40 54 C50 48 50 34 44 26 Z" fill="url(#br)"/>'
    '<path d="M16 54 L48 54 L46 58 L18 58 Z" fill="url(#br)"/>'
    '<path d="M22 26 C22 18 42 18 42 26 Z" fill="url(#br)"/>'
    '<path d="M30 18 C30 12 34 12 34 18 Z" fill="url(#br)"/></g>'
    '<path d="M18 31 C12 32 11 43 18 45" fill="none" stroke="#d8a038" stroke-width="2"/>'
    '<g fill="none" stroke="#6a4410" stroke-width="1.2" opacity=".7"><path d="M17 38 L47 38"/><path d="M20 46 C26 44 38 44 44 46"/></g>'
    f'<g {o(2)}><path d="M4 50 C4 40 12 36 16 40 C16 48 10 52 4 50 Z" fill="url(#tl)"/></g>'
    '<path d="M5 49 C8 45 11 42 15 40" fill="none" stroke="#2a6a18" stroke-width="1.2"/>'
    + hl('M22 32 C20 38 21 44 25 49', 2.2, .75) + hl('M25 22 C28 20 32 20 35 20', 1.6, .8))

# Carpets: a red carpet, rolled at one end, its border and medallion showing, fringed.
I['isle_carpets'] = svg(lg('cp', (0, '#e04a3a'), (.6, '#9a1a1a'), (1, '#5a0a0a')) + lg('cr', (0, '#d8443a', ), (.5, '#7a1212'), (1, '#3a0606'), x2=0, y2=1) + GOLD,
    shadow(32, 56, 30, 4) +
    f'<g {O}><path d="M4 50 L14 30 L58 30 L48 50 Z" fill="url(#cp)"/></g>'
    '<path d="M9 47 L17 33 L54 33 L46 47 Z" fill="none" stroke="#f0c050" stroke-width="2"/>'
    '<path d="M12 45 L19 35 L51 35 L44 45 Z" fill="none" stroke="#1a2a5a" stroke-width="1.6"/>'
    '<path d="M31 35 L38 40 L31 45 L24 40 Z" fill="url(#gd)" stroke="#1a2a5a" stroke-width="1.4"/>'
    '<g stroke="#f0e0b0" stroke-width="1.4" stroke-linecap="round">'
    + ''.join(f'<path d="M{6+k*4.4:.1f} {50} l-1 4"/>' for k in range(10)) + '</g>'
    f'<g {O}><path d="M14 30 L58 30 L58 18 L14 18 Z" fill="url(#cr)"/>'
    '<ellipse cx="14" cy="24" rx="5" ry="6" fill="#7a1212"/><ellipse cx="58" cy="24" rx="5" ry="6" fill="url(#cr)"/></g>'
    '<path d="M58 24 m-3 0 a3 3.6 0 1 0 6 0 a2 2.4 0 1 0 -4 0" fill="none" stroke="#f0c050" stroke-width="1.4"/>'
    '<g stroke="#f0c050" stroke-width="1.4" opacity=".8"><path d="M14 21 L58 21"/><path d="M14 27 L58 27"/></g>'
    + hl('M17 20 L55 20', 1.6, .6))

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
for n, s in I.items():
    open(os.path.join(out, n + '.svg'), 'w').write(s)
print(len(I))
