# Painted-style icons for the town economy's house card, in the same hand as make_icons.py:
# a face per mood (isle_mood_*), the four need categories (isle_cat_*), moving a house up
# or down (isle_move_up, isle_move_down) and holding an island (isle_hold).
# Usage: python3 make_mood_icons.py <out dir>
import os, sys
O = 'stroke="#2a1708" stroke-width="2.4" stroke-linejoin="round" stroke-linecap="round"'
def o(w):
    return f'stroke="#2a1708" stroke-width="{w}" stroke-linejoin="round" stroke-linecap="round"'
def lg(id, *stops, x1=0, y1=0, x2=0, y2=1):
    st = ''.join(f'<stop offset="{p}" stop-color="{c}"/>' for p, c in stops)
    return f'<linearGradient id="{id}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">{st}</linearGradient>'
def rg(id, *stops, cx=.38, cy=.32, r=.75):
    st = ''.join(f'<stop offset="{p}" stop-color="{c}"/>' for p, c in stops)
    return f'<radialGradient id="{id}" cx="{cx}" cy="{cy}" r="{r}">{st}</radialGradient>'
SH = '<radialGradient id="sh" cx=".5" cy=".5" r=".5"><stop offset="0" stop-color="#000" stop-opacity=".45"/><stop offset="1" stop-color="#000" stop-opacity="0"/></radialGradient>'
def shadow(cx=32, cy=58, rx=22, ry=3.5):
    return f'<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="url(#sh)"/>'
def svg(defs, body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs>{SH}{defs}</defs>{body}</svg>'

INK = '#2a1708'

# A face: a glazed disc in the mood's colour, with eyes, brows and a mouth.
def face(light, mid, dark, eyes, mouth, extra=''):
    d = rg('fc', (0, light), (.55, mid), (1, dark)) + lg('rim', (0, '#fff6d8'), (1, '#b07a30'))
    return svg(d,
        shadow(32, 60, 20, 3) +
        f'<circle cx="32" cy="31" r="26" fill="url(#rim)" {o(2.4)}/>'
        f'<circle cx="32" cy="31" r="21.5" fill="url(#fc)" {o(1.8)}/>'
        '<path d="M17 22 C20 14 28 11 34 11" stroke="#fff" stroke-width="2.6" fill="none" opacity=".55" stroke-linecap="round"/>' +
        eyes + mouth + extra)

def eyes_round(y=28, r=2.8):
    return f'<g fill="{INK}"><ellipse cx="24.5" cy="{y}" rx="{r * .85}" ry="{r}"/><ellipse cx="39.5" cy="{y}" rx="{r * .85}" ry="{r}"/></g>' \
           f'<g fill="#fff" opacity=".8"><circle cx="25.4" cy="{y - 1}" r=".9"/><circle cx="40.4" cy="{y - 1}" r=".9"/></g>'
def mouth(d, w=2.8):
    return f'<path d="{d}" stroke="{INK}" stroke-width="{w}" fill="none" stroke-linecap="round" stroke-linejoin="round"/>'

I = {}
I['isle_mood_angry'] = face('#ffb08a', '#d8502e', '#7a1a0c',
    eyes_round(30, 2.5) +
    mouth('M18.5 22 L29 26.5 M45.5 22 L35 26.5', 3.2),
    f'<path d="M22 44 C26 38.5 38 38.5 42 44" stroke="{INK}" stroke-width="3" fill="none" stroke-linecap="round"/>',
    f'<g {o(1.6)} fill="#e8e2d8"><path d="M6 12 C3 8 7 4 10 6 C11 2 16 3 15 7 C18 8 17 12 13 12 Z"/>'
    '<path d="M50 9 C48 5 52 2 55 4 C57 1 61 3 60 7 C63 8 62 12 58 11 Z"/></g>')
I['isle_mood_unhappy'] = face('#ffd8a0', '#e8963a', '#8a4a14',
    eyes_round(29, 2.6) +
    mouth('M19 22.5 C21 21 24 20.5 28 21.5 M45 22.5 C43 21 40 20.5 36 21.5', 2.4),
    mouth('M23 43 C27 39.5 37 39.5 41 43'),
    '<path d="M41 33 C42.5 36 43.5 37.5 42 39 C40.5 40 39 38.5 39.6 36.8 Z" fill="#7ac8f0" stroke="#2a1708" stroke-width="1.2"/>')
I['isle_mood_content'] = face('#fff4c0', '#e8c25a', '#8a6a1c',
    eyes_round(28.5, 2.7),
    mouth('M23.5 41 L40.5 41'))
I['isle_mood_happy'] = face('#e8ffc0', '#8ec04a', '#3a6a1a',
    eyes_round(28, 2.8),
    mouth('M21.5 37.5 C25 44.5 39 44.5 42.5 37.5', 3))
I['isle_mood_euphoric'] = face('#d8ffd0', '#4ab852', '#1a5a24',
    f'<g {o(2.8)} fill="none"><path d="M20 29 C22 25 27 25 29 29"/><path d="M35 29 C37 25 42 25 44 29"/></g>',
    f'<path d="M20 35 C22 47 42 47 44 35 Z" fill="#7a1a14" {o(2.4)}/>'
    '<path d="M24 42 C28 39 36 39 40 42 C37 45 27 45 24 42 Z" fill="#e8706a"/>'
    '<path d="M21.5 35.5 L42.5 35.5 L41 38 L23 38 Z" fill="#fff"/>',
    '<g fill="#fff4a0" stroke="#8a6a1c" stroke-width="1.1" stroke-linejoin="round">'
    '<path d="M8 10 L9.6 14.4 L14 16 L9.6 17.6 L8 22 L6.4 17.6 L2 16 L6.4 14.4 Z"/>'
    '<path d="M56 4 L57.2 7.8 L61 9 L57.2 10.2 L56 14 L54.8 10.2 L51 9 L54.8 7.8 Z"/>'
    '<path d="M57 42 L58 45 L61 46 L58 47 L57 50 L56 47 L53 46 L56 45 Z"/></g>')

# The categories.
BREAD = lg('br', (0, '#f8d898'), (.5, '#d8963a'), (1, '#8a4a14'))
FISH = lg('fi', (0, '#c8e0ec'), (.5, '#7a9aac'), (1, '#3a5a6c'))
I['isle_cat_food'] = svg(BREAD + FISH,
    shadow(32, 58, 26, 3.5) +
    f'<g {O}><path d="M6 36 C10 24 26 20 40 22 C50 24 58 30 58 38 C58 46 50 50 32 50 C16 50 4 46 6 36 Z" fill="url(#br)"/></g>'
    '<g stroke="#6a3410" stroke-width="2.2" fill="none" stroke-linecap="round"><path d="M18 30 C20 34 20 38 18 42"/><path d="M28 27 C30 32 30 38 28 44"/><path d="M38 27 C40 32 40 38 38 44"/><path d="M48 30 C50 34 50 38 48 42"/></g>'
    '<path d="M12 33 C16 26 26 24 34 24" stroke="#fff4d8" stroke-width="2" fill="none" opacity=".75" stroke-linecap="round"/>'
    f'<g {o(2)}><path d="M14 50 C20 42 34 42 42 48 L52 42 L50 50 L52 58 L42 52 C34 58 20 58 14 50 Z" fill="url(#fi)"/></g>'
    f'<circle cx="20" cy="49" r="1.6" fill="{INK}"/>'
    '<path d="M26 46 C28 49 28 52 26 54 M32 46 C34 49 34 52 32 54" stroke="#2a3a48" stroke-width="1.2" fill="none" opacity=".6"/>')
COAT = lg('ct', (0, '#6a8ac8'), (.55, '#3a5a9a'), (1, '#1a2a5a'))
I['isle_cat_clothing'] = svg(COAT + lg('ln', (0, '#f8f0dc'), (1, '#c8b890')),
    shadow(32, 60, 22, 3) +
    f'<g {O}><path d="M22 8 L32 14 L42 8 L54 14 L60 34 L52 36 L48 26 L48 56 L16 56 L16 26 L12 36 L4 34 L10 14 Z" fill="url(#ct)"/></g>'
    f'<g {o(1.8)}><path d="M22 8 L32 22 L42 8 L38 7 L32 14 L26 7 Z" fill="url(#ln)"/></g>'
    f'<path d="M32 22 L32 56" {o(1.8)}/>'
    '<g fill="#e8c060" stroke="#2a1708" stroke-width="1.1"><circle cx="35" cy="30" r="1.8"/><circle cx="35" cy="38" r="1.8"/><circle cx="35" cy="46" r="1.8"/></g>'
    f'<g {o(1.6)} fill="#2a3a6a"><path d="M19 40 L28 40 L28 46 L19 46 Z"/></g>'
    '<path d="M12 16 C16 12 20 11 22 11" stroke="#a8c0f0" stroke-width="1.8" fill="none" opacity=".8" stroke-linecap="round"/>')
STONE = lg('stn', (0, '#e8e0cc'), (.55, '#b8ac90'), (1, '#6a604c'))
WATER = lg('wt', (0, '#bfe8ff'), (1, '#3a8ac8'))
I['isle_cat_square'] = svg(STONE + WATER,
    shadow(32, 59, 28, 3.5) +
    f'<g {O}><path d="M4 50 C4 44 60 44 60 50 L60 54 C60 60 4 60 4 54 Z" fill="url(#stn)"/></g>'
    f'<g {o(1.8)}><path d="M9 50 C9 46 55 46 55 50 C55 53 9 53 9 50 Z" fill="url(#wt)"/></g>'
    f'<g {O}><path d="M28 50 L28 30 L36 30 L36 50 Z" fill="url(#stn)"/>'
    '<path d="M18 30 C18 26 46 26 46 30 C46 33 18 33 18 30 Z" fill="url(#stn)"/>'
    '<path d="M29 26 L29 18 L35 18 L35 26 Z" fill="url(#stn)"/></g>'
    '<g stroke="#7ac8f0" stroke-width="2.4" fill="none" stroke-linecap="round"><path d="M32 16 C32 8 24 6 20 14 C18 18 18 24 20 28"/><path d="M32 16 C32 8 40 6 44 14 C46 18 46 24 44 28"/></g>'
    '<g stroke="#e8f8ff" stroke-width="1.1" fill="none" stroke-linecap="round" opacity=".9"><path d="M32 15 C32 9 26 8 22.5 14"/><path d="M32 15 C32 9 38 8 41.5 14"/></g>'
    f'<g {o(2)}><circle cx="32" cy="14" r="3.4" fill="#e8c060"/></g>'
    '<path d="M14 49 C20 47.5 26 47.5 30 48 M38 48 C44 47.5 48 48 52 49" stroke="#e8f8ff" stroke-width="1.2" fill="none" opacity=".8"/>')
GOLD = lg('gd', (0, '#fff0b0'), (.5, '#e0a830'), (1, '#8a5a10'), x2=1, y2=0)
I['isle_cat_luxury'] = svg(GOLD + lg('wn', (0, '#e04a5a'), (1, '#6a0a1a')),
    shadow(32, 59, 18, 3) +
    f'<g {O}><path d="M16 8 L48 8 C48 26 42 34 32 34 C22 34 16 26 16 8 Z" fill="url(#gd)"/>'
    '<path d="M29 34 L35 34 L35 46 L29 46 Z" fill="url(#gd)"/>'
    '<path d="M18 56 C18 48 46 48 46 56 Z" fill="url(#gd)"/></g>'
    f'<g {o(1.6)}><path d="M18.5 14 L45.5 14 C44.5 26 40 31 32 31 C24 31 19.5 26 18.5 14 Z" fill="url(#wn)"/></g>'
    '<path d="M21 16 C22 24 25 28 28 29" stroke="#ffb0b8" stroke-width="1.6" fill="none" opacity=".7" stroke-linecap="round"/>'
    '<g fill="#fff8c0" stroke="#8a6a1c" stroke-width="1.1" stroke-linejoin="round">'
    '<path d="M52 14 L53.4 18 L57.4 19.4 L53.4 20.8 L52 24.8 L50.6 20.8 L46.6 19.4 L50.6 18 Z"/>'
    '<path d="M10 26 L11 29 L14 30 L11 31 L10 34 L9 31 L6 30 L9 29 Z"/></g>')

# Moving up and down: a little house with a bold arrow.
WALL = lg('wl', (0, '#fff4dc'), (1, '#c8b08a'))
ROOF = lg('rf', (0, '#e0704a'), (1, '#8a2a14'))
def little_house():
    return (f'<g {O}><path d="M8 34 L22 22 L36 34 Z" fill="url(#rf)"/><path d="M11 34 L33 34 L33 54 L11 54 Z" fill="url(#wl)"/></g>'
            f'<g {o(1.6)}><path d="M18 54 L18 44 L26 44 L26 54" fill="#6a4020"/></g>')
UP = lg('up', (0, '#c8f090'), (.5, '#5aa83a'), (1, '#1a5a14'))
DOWN = lg('dn', (0, '#ffd090'), (.5, '#d07a2a'), (1, '#7a3a0c'))
I['isle_move_up'] = svg(WALL + ROOF + UP,
    shadow(32, 58, 26, 3) + little_house() +
    f'<g {O}><path d="M48 4 L62 22 L54 22 L54 54 L42 54 L42 22 L34 22 Z" fill="url(#up)"/></g>'
    '<path d="M48 9 L57 20.5" stroke="#efffd0" stroke-width="1.8" fill="none" opacity=".8" stroke-linecap="round"/>')
I['isle_move_down'] = svg(WALL + ROOF + DOWN,
    shadow(32, 58, 26, 3) + little_house() +
    f'<g {O}><path d="M48 56 L62 38 L54 38 L54 6 L42 6 L42 38 L34 38 Z" fill="url(#dn)"/></g>'
    '<path d="M44 9 L44 36" stroke="#fff0c8" stroke-width="1.8" fill="none" opacity=".8" stroke-linecap="round"/>')
# Holding an island: a brass padlock over an up arrow.
IRON = lg('ir', (0, '#b8c0c8'), (.5, '#6a737c'), (1, '#2c3238'))
I['isle_hold'] = svg(UP + GOLD + IRON,
    shadow(32, 59, 22, 3) +
    f'<g {O} opacity=".9"><path d="M32 3 L48 20 L39 20 L39 30 L25 30 L25 20 L16 20 Z" fill="url(#up)"/></g>'
    f'<path d="M22 34 L22 26 C22 18 42 18 42 26 L42 34" fill="none" {o(7)}/>'
    '<path d="M22 34 L22 26 C22 18 42 18 42 26 L42 34" fill="none" stroke="url(#ir)" stroke-width="3.6" stroke-linecap="round"/>'
    f'<g {O}><rect x="13" y="32" width="38" height="24" rx="4" fill="url(#gd)"/></g>'
    f'<g {o(1.4)}><circle cx="32" cy="42" r="3.6" fill="{INK}"/><path d="M30.4 44 L29.4 51 L34.6 51 L33.6 44 Z" fill="{INK}"/></g>'
    '<path d="M16 36 L48 36" stroke="#fff4c8" stroke-width="1.6" opacity=".7"/>')

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
for n, s in I.items():
    open(os.path.join(out, n + '.svg'), 'w').write(s)
print(len(I))
