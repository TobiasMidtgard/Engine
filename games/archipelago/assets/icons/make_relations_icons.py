# Painted-style icons for the relations screen, in the same hand as make_icons.py: the rival
# houses' emblems (isle_emblem_gull, isle_emblem_lantern, isle_emblem_anvil, and for the new
# characters isle_emblem_compass, isle_emblem_owl, isle_emblem_cutlass, isle_emblem_rose,
# isle_emblem_scales), the treaties
# (isle_treaty_war, isle_treaty_hostile, isle_treaty_neutral, isle_treaty_pact,
# isle_treaty_alliance) and the three sides of a relation (isle_rel_diplomacy,
# isle_rel_trade, isle_rel_military), and a gift (isle_gift).
# Usage: python3 make_relations_icons.py <out dir>
import os, sys
O = 'stroke="#2a1708" stroke-width="2.4" stroke-linejoin="round" stroke-linecap="round"'
def o(w):
    return f'stroke="#2a1708" stroke-width="{w}" stroke-linejoin="round" stroke-linecap="round"'
def lg(id, *stops, x1=0, y1=0, x2=0, y2=1):
    st = ''.join(f'<stop offset="{p}" stop-color="{c}"/>' for p, c in stops)
    return f'<linearGradient id="{id}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">{st}</linearGradient>'
SH = '<radialGradient id="sh" cx=".5" cy=".5" r=".5"><stop offset="0" stop-color="#000" stop-opacity=".45"/><stop offset="1" stop-color="#000" stop-opacity="0"/></radialGradient>'
def shadow(cx=32, cy=58, rx=22, ry=3.5):
    return f'<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="url(#sh)"/>'
def svg(defs, body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs>{SH}{defs}</defs>{body}</svg>'
INK = '#2a1708'
GOLD = lg('gd', (0, '#fff0b0'), (.5, '#e0a830'), (1, '#8a5a10'))
IRON = lg('ir', (0, '#d8dee4'), (.5, '#8a939c'), (1, '#3c4248'))
WOOD = lg('wd', (0, '#d8a46a'), (.55, '#9a6432'), (1, '#5a3414'))
PAPER = lg('pp', (0, '#fffaf0'), (.6, '#efe2c0'), (1, '#c8b48a'))
I = {}

# Emblems: white charges, drawn to sit on a shield in the house's colour (the screen paints
# the shield), so each reads on any colour.
CH = lg('ch', (0, '#ffffff'), (1, '#d8d0c0'))
I['isle_emblem_gull'] = svg(CH,
    f'<g {o(2.6)}><path d="M4 30 C14 20 24 22 32 32 C40 22 50 20 60 30 C52 28 44 30 38 38 L32 44 L26 38 C20 30 12 28 4 30 Z" fill="url(#ch)"/></g>'
    f'<g {o(1.8)}><path d="M32 32 L35 30 L32 38 L29 30 Z" fill="#f0b040"/></g>'
    f'<circle cx="32" cy="28.5" r="2" fill="{INK}"/>')
I['isle_emblem_lantern'] = svg(CH + lg('fl', (0, '#fff8c0'), (1, '#f0a020')),
    f'<g {o(2.4)}><path d="M26 10 C26 4 38 4 38 10" fill="none"/><path d="M22 14 L42 14 L38 18 L26 18 Z" fill="url(#ch)"/>'
    '<path d="M24 18 L40 18 L42 46 L22 46 Z" fill="url(#ch)"/><path d="M20 46 L44 46 L40 54 L24 54 Z" fill="url(#ch)"/></g>'
    f'<g {o(1.6)}><path d="M32 24 C36 30 37 34 32 40 C27 34 28 30 32 24 Z" fill="url(#fl)"/></g>'
    '<path d="M27 20 L26 44 M37 20 L38 44" stroke="#2a1708" stroke-width="1.6"/>')
I['isle_emblem_anvil'] = svg(CH,
    f'<g {o(2.6)}><path d="M6 18 L44 18 C50 18 56 20 60 24 C54 26 48 26 44 28 L42 34 L36 36 L40 48 L46 50 L46 56 L18 56 L18 50 L24 48 L28 36 L20 34 L18 28 C12 26 8 22 6 18 Z" fill="url(#ch)"/></g>'
    '<path d="M10 20 L42 20" stroke="#fff" stroke-width="1.6" opacity=".8"/>'
    f'<g {o(2)}><path d="M48 4 L52 12 M56 6 L54 13 M40 6 L46 13" fill="none"/></g>')

# A compass rose: four long points and four short, a ring and a pin.
I['isle_emblem_compass'] = svg(CH,
    f'<g {o(2.2)}><circle cx="32" cy="32" r="16" fill="none"/>'
    '<path d="M32 2 L37 27 L62 32 L37 37 L32 62 L27 37 L2 32 L27 27 Z" fill="url(#ch)"/>'
    '</g>'
    f'<g {o(1.8)}><path d="M32 32 L45 19 L36 32 Z M32 32 L45 45 L32 36 Z M32 32 L19 45 L28 32 Z M32 32 L19 19 L32 28 Z" fill="url(#ch)"/></g>'
    f'<g {o(2.2)}><path d="M32 2 L37 27 L62 32 L37 37 L32 62 L27 37 L2 32 L27 27 Z" fill="url(#ch)"/></g>'
    '<path d="M32 4 L36 28 L32 32 Z" fill="#2a1708" opacity=".35"/>'
    f'<circle cx="32" cy="32" r="3" fill="{INK}"/>')
# An owl, face on: tufted ears, round eyes, folded wings.
I['isle_emblem_owl'] = svg(CH + lg('ey', (0, '#fff0a0'), (1, '#e0a020')),
    f'<g {o(2.4)}><path d="M16 12 L22 20 C28 18 36 18 42 20 L48 12 L50 28 C54 40 48 56 32 58 C16 56 10 40 14 28 Z" fill="url(#ch)"/>'
    '<circle cx="24" cy="30" r="7" fill="url(#ey)"/><circle cx="40" cy="30" r="7" fill="url(#ey)"/>'
    '<path d="M29 36 L32 42 L35 36 Z" fill="#f0b040"/></g>'
    f'<circle cx="24" cy="30" r="3" fill="{INK}"/><circle cx="40" cy="30" r="3" fill="{INK}"/>'
    '<g fill="none" stroke="#2a1708" stroke-width="1.6"><path d="M20 44 C24 50 28 52 32 52 C36 52 40 50 44 44"/><path d="M24 48 L26 46 M30 50 L32 48 M36 50 L38 48"/></g>')
# A cutlass, point up, with a basket guard.
I['isle_emblem_cutlass'] = svg(CH + GOLD,
    f'<g {o(2.4)}><path d="M30 42 C26 30 28 16 40 4 C38 18 38 30 36 42 Z" fill="url(#ch)"/>'
    '<path d="M20 42 L44 42 L44 46 L20 46 Z" fill="url(#gd)"/>'
    '<path d="M44 44 C50 46 50 56 40 58 L28 58" fill="none"/>'
    '<path d="M30 46 L34 46 L34 58 L30 58 Z" fill="url(#gd)"/></g>'
    '<path d="M33 40 C32 28 34 16 39 7" fill="none" stroke="#a8a090" stroke-width="1.4"/>')
# A rose, open, on a short stem with two leaves.
I['isle_emblem_rose'] = svg(CH,
    f'<g {o(2.2)}><path d="M32 40 L32 60" fill="none"/>'
    '<path d="M32 50 C24 44 16 48 14 54 C22 56 28 54 32 50 Z M32 46 C40 40 48 44 50 50 C42 52 36 50 32 46 Z" fill="url(#ch)"/>'
    '<path d="M32 6 C44 6 52 14 50 26 C48 36 40 42 32 42 C24 42 16 36 14 26 C12 14 20 6 32 6 Z" fill="url(#ch)"/></g>'
    '<g fill="none" stroke="#2a1708" stroke-width="1.8" stroke-linecap="round"><path d="M32 14 C26 16 24 22 28 26 C32 30 38 26 36 20"/><path d="M22 20 C20 28 24 34 32 34 C40 34 44 28 42 20"/><path d="M18 28 C20 36 26 40 32 40"/></g>')
# The scales of a counting house: a beam on a post, two pans.
I['isle_emblem_scales'] = svg(CH,
    f'<g {o(2.4)}><path d="M30 10 L34 10 L34 52 L30 52 Z" fill="url(#ch)"/>'
    '<path d="M20 52 L44 52 L46 58 L18 58 Z" fill="url(#ch)"/>'
    '<path d="M8 16 L56 16 L56 20 L8 20 Z" fill="url(#ch)"/>'
    '<circle cx="32" cy="10" r="4" fill="url(#ch)"/>'
    '<path d="M2 38 C4 46 20 46 22 38 Z M42 38 C44 46 60 46 62 38 Z" fill="url(#ch)"/></g>'
    '<g fill="none" stroke="#2a1708" stroke-width="1.6"><path d="M12 20 L2 38 M12 20 L22 38 M52 20 L42 38 M52 20 L62 38"/></g>')

# Treaties.
I['isle_treaty_war'] = svg(IRON + WOOD + lg('rd', (0, '#ff8a6a'), (1, '#9a2414')),
    shadow(32, 59, 22, 3) +
    f'<g {O}><path d="M10 6 L14 4 L50 44 L46 48 Z" fill="url(#ir)"/><path d="M54 6 L50 4 L14 44 L18 48 Z" fill="url(#ir)"/></g>'
    f'<g {o(2.2)}><path d="M40 40 L50 50 M44 36 L54 46" fill="none"/><path d="M24 40 L14 50 M20 36 L10 46" fill="none"/></g>'
    f'<g {o(2)}><path d="M46 46 L56 56 L60 52 L50 42 Z" fill="url(#wd)"/><path d="M18 46 L8 56 L4 52 L14 42 Z" fill="url(#wd)"/></g>'
    f'<g {o(2)}><circle cx="32" cy="26" r="7" fill="url(#rd)"/></g>')
I['isle_treaty_hostile'] = svg(lg('st', (0, '#ffb070'), (1, '#c0502a')),
    shadow(32, 59, 18, 3) +
    f'<g {O}><path d="M32 4 L60 54 L4 54 Z" fill="url(#st)"/></g>'
    f'<g {o(1.5)} fill="{INK}"><path d="M29 18 L35 18 L34 40 L30 40 Z"/><circle cx="32" cy="47" r="3.2"/></g>')
I['isle_treaty_neutral'] = svg(lg('nt', (0, '#f4eee0'), (1, '#a8a090')) + WOOD,
    shadow(32, 59, 18, 3) +
    f'<g {O}><path d="M30 6 L34 6 L34 56 L30 56 Z" fill="url(#wd)"/>'
    '<path d="M34 8 C42 6 48 12 56 10 L56 30 C48 32 42 26 34 28 Z" fill="url(#nt)"/></g>'
    '<path d="M38 12 C44 11 48 15 52 14" stroke="#fff" stroke-width="1.6" fill="none" opacity=".8"/>')
I['isle_treaty_pact'] = svg(PAPER + lg('sl', (0, '#e85a4a'), (1, '#8a1a14')),
    shadow(32, 59, 24, 3) +
    f'<g {O}><path d="M10 10 L50 10 C56 10 56 18 50 18 L50 52 L14 52 C8 52 8 44 14 44 L14 18 C8 18 8 10 14 10 Z" fill="url(#pp)"/></g>'
    '<g stroke="#8a6a3a" stroke-width="1.8" stroke-linecap="round"><path d="M20 24 L44 24 M20 30 L44 30 M20 36 L36 36"/></g>'
    f'<g {o(2)}><circle cx="42" cy="44" r="7.5" fill="url(#sl)"/></g>'
    '<path d="M38 50 L36 58 L40 56 L42 59 L43 51 M46 50 L48 58" stroke="#8a1a14" stroke-width="2.4" fill="none" stroke-linejoin="round"/>')
I['isle_treaty_alliance'] = svg(GOLD + lg('bl', (0, '#6a8ac8'), (1, '#1a2a5a')) + lg('gr', (0, '#8ac86a'), (1, '#1a5a24')),
    shadow(32, 60, 20, 3) +
    f'<g {O}><path d="M32 4 L56 12 C56 36 48 50 32 58 C16 50 8 36 8 12 Z" fill="url(#gd)"/></g>'
    f'<g {o(1.6)}><path d="M32 9 L32 53 C20 46 13 34 13 15 Z" fill="url(#bl)"/><path d="M32 9 L51 15 C51 34 44 46 32 53 Z" fill="url(#gr)"/></g>'
    f'<g {o(1.8)} fill="#fff4c8"><path d="M24 30 C26 26 30 26 32 29 C34 26 38 26 40 30 L36 36 L28 36 Z"/></g>')

# The three sides of a relation.
I['isle_rel_diplomacy'] = svg(PAPER + GOLD,
    shadow(32, 59, 22, 3) +
    f'<g {O}><path d="M8 20 L32 10 L56 20 L56 26 L8 26 Z" fill="url(#gd)"/>'
    '<path d="M12 26 L18 26 L18 48 L12 48 Z M23 26 L29 26 L29 48 L23 48 Z M35 26 L41 26 L41 48 L35 48 Z M46 26 L52 26 L52 48 L46 48 Z" fill="url(#pp)"/>'
    '<path d="M6 48 L58 48 L58 54 L6 54 Z" fill="url(#gd)"/></g>')
I['isle_rel_trade'] = svg(GOLD + WOOD,
    shadow(32, 59, 24, 3) +
    f'<g {O}><path d="M32 6 L32 48" stroke-width="5"/><path d="M32 6 L32 48" stroke="url(#gd)" stroke-width="2"/>'
    '<path d="M10 16 L54 16" stroke-width="5"/><path d="M10 16 L54 16" stroke="url(#gd)" stroke-width="2"/>'
    '<path d="M4 34 L16 34 L10 18 Z M48 34 L60 34 L54 18 Z" fill="none" stroke-width="1.6"/>'
    '<path d="M2 34 C4 42 18 42 20 34 Z M44 34 C46 42 60 42 62 34 Z" fill="url(#gd)"/>'
    '<path d="M22 48 L42 48 L46 56 L18 56 Z" fill="url(#wd)"/></g>'
    f'<g {o(1.4)}><circle cx="9" cy="32" r="3" fill="#f0c860"/><circle cx="14" cy="31" r="3" fill="#f0c860"/></g>')
I['isle_rel_military'] = svg(IRON + WOOD,
    shadow(32, 59, 24, 3) +
    f'<g {O}><path d="M8 40 L44 40 L52 30 L56 30 L56 40 L60 40 L54 50 L12 50 Z" fill="url(#wd)"/>'
    '<path d="M16 40 L16 28 L40 28 L40 40" fill="url(#ir)"/></g>'
    f'<g {o(2.6)}><path d="M40 32 L60 22" stroke-width="7"/><path d="M40 32 L60 22" stroke="url(#ir)" stroke-width="3.6"/></g>'
    '<g fill="#6a6a68" opacity=".7"><circle cx="58" cy="16" r="4"/><circle cx="52" cy="10" r="3"/></g>'
    f'<g {o(1.4)} fill="#2a1e14"><circle cx="22" cy="45" r="2"/><circle cx="30" cy="45" r="2"/><circle cx="38" cy="45" r="2"/></g>')
I['isle_gift'] = svg(GOLD + lg('bx', (0, '#c0503a'), (1, '#6a2414')),
    shadow(32, 59, 22, 3) +
    f'<g {O}><path d="M10 28 L54 28 L54 56 L10 56 Z" fill="url(#bx)"/><path d="M6 20 L58 20 L58 30 L6 30 Z" fill="url(#bx)"/>'
    '<path d="M28 20 L36 20 L36 56 L28 56 Z" fill="url(#gd)"/></g>'
    f'<g {o(2)}><path d="M32 20 C24 6 12 10 18 18 C20 20 26 20 32 20 Z" fill="url(#gd)"/><path d="M32 20 C40 6 52 10 46 18 C44 20 38 20 32 20 Z" fill="url(#gd)"/></g>')

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
for n, s in I.items():
    open(os.path.join(out, n + '.svg'), 'w').write(s)
print(len(I))
