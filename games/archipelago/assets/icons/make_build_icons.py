# Painted-style icons for the build bar's tier tabs, in the same hand as make_icons.py: a
# fisher's oilskin sou'wester (isle_tier_fisherfolk), a labourer's flat cap over a hammer
# (isle_tier_labourers) and a burgher's top hat (isle_tier_burghers); and the pedlar's stall
# (isle_pedlar): a striped awning over an open chest of wares; and a padlock for buildings
# not open yet (isle_padlock).
# Usage: python3 make_build_icons.py <out dir>
import os, sys
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
WOOD = lg('wd', (0, '#d8a46a'), (.55, '#9a6432'), (1, '#5a3414'))
IRON = lg('ir', (0, '#d8dee4'), (.5, '#8a939c'), (1, '#3c4248'))
I = {}

# Sou'wester: a yellow oilskin hat, round crown and a wide brim longer at the back.
I['isle_tier_fisherfolk'] = svg(lg('oy', (0, '#fff0a0'), (.5, '#f2c22a'), (1, '#a8740c')),
    shadow(32, 58, 26, 3.5) +
    f'<g {o(2.6)}><path d="M4 44 C8 36 18 36 32 37 C46 36 56 36 60 46 C58 52 50 52 44 48 C36 44 28 44 20 47 C12 50 4 50 4 44 Z" fill="url(#oy)"/>'
    '<path d="M16 40 C14 24 22 12 32 12 C42 12 50 24 48 40 C40 37 24 37 16 40 Z" fill="url(#oy)"/></g>'
    f'<g {o(1.6)} fill="none"><path d="M18 34 C26 31 38 31 46 34"/><path d="M32 12 L32 22"/></g>'
    '<path d="M22 20 C24 16 28 14 31 14" stroke="#fff" stroke-width="2.4" fill="none" opacity=".7" stroke-linecap="round"/>')

# Flat cap over a claw hammer.
I['isle_tier_labourers'] = svg(lg('cp', (0, '#a8a090'), (.5, '#6a6252'), (1, '#3a3428')) + WOOD + IRON,
    shadow(32, 59, 24, 3.5) +
    f'<g {o(2.6)}><path d="M6 40 C6 24 18 16 32 16 C44 16 52 22 54 32 C58 34 62 38 60 42 C50 46 20 46 6 40 Z" fill="url(#cp)"/></g>'
    f'<g {o(1.6)} fill="none"><path d="M10 38 C24 41 40 41 54 34"/><path d="M30 18 C28 26 28 32 30 39"/></g>'
    '<path d="M14 28 C18 22 24 19 30 19" stroke="#fff" stroke-width="2.2" fill="none" opacity=".5" stroke-linecap="round"/>'
    f'<g {o(2.4)}><path d="M14 56 L44 26 L48 30 L18 60 Z" fill="url(#wd)"/>'
    '<path d="M38 20 L50 8 C54 10 56 12 58 16 L46 28 Z" fill="url(#ir)"/></g>')

# Top hat with a red band and a brim.
I['isle_tier_burghers'] = svg(lg('th', (0, '#5a5a6a'), (.4, '#2a2a34'), (1, '#0c0c12')) + lg('bd', (0, '#e85a4a'), (1, '#7a1a14')),
    shadow(32, 59, 26, 3.5) +
    f'<g {o(2.6)}><path d="M4 50 C4 44 18 42 32 42 C46 42 60 44 60 50 C60 55 46 56 32 56 C18 56 4 55 4 50 Z" fill="url(#th)"/>'
    '<path d="M16 46 L18 8 C26 5 38 5 46 8 L48 46 C38 48 26 48 16 46 Z" fill="url(#th)"/></g>'
    f'<g {o(1.8)}><path d="M17.4 36 C27 38 37 38 46.6 36 L47 44 C37 46 27 46 17 44 Z" fill="url(#bd)"/></g>'
    '<path d="M23 12 L22 32" stroke="#fff" stroke-width="2.6" opacity=".35" stroke-linecap="round"/>'
    '<ellipse cx="32" cy="8" rx="13" ry="2.6" fill="#3a3a48" stroke="#2a1708" stroke-width="1.6"/>')

# The pedlar's stall: a red and cream awning on two posts over an open chest with a gem.
I['isle_pedlar'] = svg(WOOD + lg('aw', (0, '#ff7a5a'), (.6, '#c8321e'), (1, '#7a160c')) + lg('cr', (0, '#fff6dc'), (1, '#d8c08a'))
    + lg('gd', (0, '#fff2a0'), (.5, '#e8b030'), (1, '#8a5a10')) + lg('gm', (0, '#a8f0ff'), (.5, '#2a9ad8'), (1, '#0a3a7a')),
    shadow(32, 59, 26, 3.5) +
    f'<g {o(2.4)}><path d="M10 22 L12 56 L16 56 L15 22 Z" fill="url(#wd)"/><path d="M54 22 L52 56 L48 56 L49 22 Z" fill="url(#wd)"/></g>'
    # Awning: a sloped roof of five stripes with a scalloped edge.
    f'<g {o(2.4)}><path d="M6 22 L14 6 L50 6 L58 22 Z" fill="url(#cr)"/></g>'
    '<path d="M14 6 L20.7 6 L16.4 22 L6 22 Z M28 6 L35.5 6 L37 22 L27 22 Z M43 6 L50 6 L58 22 L47.6 22 Z" fill="url(#aw)"/>'
    f'<g {o(2.4)}><path d="M6 22 C6 28 16.4 28 16.4 22 C16.4 28 27 28 27 22 C27 28 37 28 37 22 C37 28 47.6 28 47.6 22 C47.6 28 58 28 58 22 Z" fill="url(#aw)"/></g>'
    f'<g {o(2.4)} fill="none"><path d="M6 22 L14 6 L50 6 L58 22 Z"/></g>'
    # The chest: open lid behind, gold and a gem heaped in it.
    f'<g {o(2.4)}><path d="M18 40 L20 32 L44 32 L46 40 Z" fill="url(#wd)"/>'
    '<path d="M20 40 C22 34 30 33 32 35 C35 32 42 34 44 40 Z" fill="url(#gd)"/>'
    '<path d="M28 39 L32 33 L36 39 L32 43 Z" fill="url(#gm)"/>'
    '<path d="M16 40 L48 40 L46 56 L18 56 Z" fill="url(#wd)"/></g>'
    f'<g {o(1.8)} fill="none"><path d="M17 46 L47 46"/></g>'
    '<rect x="29" y="44" width="6" height="6" rx="1" fill="url(#gd)" stroke="#2a1708" stroke-width="1.6"/>'
    '<path d="M30 35.5 L32 33.5" stroke="#fff" stroke-width="1.6" opacity=".8" stroke-linecap="round"/>')

# A padlock: an iron shackle over a brass body with a keyhole.
I['isle_padlock'] = svg(IRON + lg('br', (0, '#ffe7a0'), (.5, '#d0a040'), (1, '#6a4410')),
    shadow(32, 59, 18, 3) +
    f'<g {o(2.6)}><path d="M18 30 L18 20 C18 10 24 5 32 5 C40 5 46 10 46 20 L46 30 L39 30 L39 20 C39 14 36 11 32 11 C28 11 25 14 25 20 L25 30 Z" fill="url(#ir)"/>'
    '<rect x="11" y="28" width="42" height="29" rx="5" fill="url(#br)"/></g>'
    '<path d="M32 36 C29 36 27.5 38 27.5 40.5 C27.5 42.5 28.6 43.8 30 44.4 L29 51 L35 51 L34 44.4 C35.4 43.8 36.5 42.5 36.5 40.5 C36.5 38 35 36 32 36 Z" fill="#2a1708"/>'
    '<path d="M15 33 L15 46" stroke="#fff" stroke-width="2.4" opacity=".55" stroke-linecap="round"/>')

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
for n, s in I.items():
    open(os.path.join(out, n + '.svg'), 'w').write(s)
print(len(I))
