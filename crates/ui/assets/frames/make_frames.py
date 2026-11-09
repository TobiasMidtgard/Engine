# Generates the brass kit's nine-slice frames and ornaments (SVG) for crates/ui/assets/frames.
import os, sys
out = sys.argv[1]
os.makedirs(out, exist_ok=True)

BRASS_DEFS = '''<linearGradient id="br" x1="0" y1="0" x2="1" y2="1">
<stop offset="0" stop-color="#fbe7a8"/><stop offset=".35" stop-color="#d9ad55"/><stop offset=".7" stop-color="#a87a2c"/><stop offset="1" stop-color="#6e4a16"/></linearGradient>
<radialGradient id="ros" cx=".38" cy=".34" r=".75"><stop offset="0" stop-color="#fff3c8"/><stop offset=".35" stop-color="#e2b65d"/><stop offset=".8" stop-color="#8f6421"/><stop offset="1" stop-color="#4a300c"/></radialGradient>
<radialGradient id="gem" cx=".4" cy=".35" r=".7"><stop offset="0" stop-color="#9ee7f0"/><stop offset=".45" stop-color="#2b8fa3"/><stop offset="1" stop-color="#0c3440"/></radialGradient>'''

def mirror4(g, size):
    s = size
    return (f'<g>{g}</g>'
            f'<g transform="translate({s},0) scale(-1,1)">{g}</g>'
            f'<g transform="translate(0,{s}) scale(1,-1)">{g}</g>'
            f'<g transform="translate({s},{s}) scale(-1,-1)">{g}</g>')

def frame_brass():
    S = 128
    # Edges: a dark outline band, the brass band with a light top lip, and an inner thin line.
    edges = f'''
<rect x="3" y="3" width="{S-6}" height="{S-6}" rx="3" fill="none" stroke="#1c1105" stroke-width="7"/>
<rect x="3" y="3" width="{S-6}" height="{S-6}" rx="3" fill="none" stroke="#b98a3a" stroke-width="4.2"/>
<rect x="1.9" y="1.9" width="{S-3.8}" height="{S-3.8}" rx="4" fill="none" stroke="#f6dd98" stroke-width="1" opacity=".85"/>
<rect x="4.3" y="4.3" width="{S-8.6}" height="{S-8.6}" rx="2" fill="none" stroke="#5a3b10" stroke-width="1" opacity=".9"/>
<rect x="10.5" y="10.5" width="{S-21}" height="{S-21}" fill="none" stroke="#1c1105" stroke-width="2.6" opacity=".55"/>
<rect x="10.5" y="10.5" width="{S-21}" height="{S-21}" fill="none" stroke="#d8b264" stroke-width="1.1"/>'''
    corner = '''
<path d="M5 44 L5 15 Q5 5 15 5 L44 5" fill="none" stroke="#1c1105" stroke-width="9" stroke-linecap="round"/>
<path d="M5 44 L5 15 Q5 5 15 5 L44 5" fill="none" stroke="url(#br)" stroke-width="5.6" stroke-linecap="round"/>
<path d="M44 16 C30 15 22 17 19.5 22 C17 27 21.5 31 25 29 C28.5 27 26.5 22.5 23.5 23.8" fill="none" stroke="#1c1105" stroke-width="4.4" stroke-linecap="round"/>
<path d="M44 16 C30 15 22 17 19.5 22 C17 27 21.5 31 25 29 C28.5 27 26.5 22.5 23.5 23.8" fill="none" stroke="url(#br)" stroke-width="2.4" stroke-linecap="round"/>
<path d="M16 44 C15 30 17 22 22 19.5 C27 17 31 21.5 29 25 C27 28.5 22.5 26.5 23.8 23.5" fill="none" stroke="#1c1105" stroke-width="4.4" stroke-linecap="round"/>
<path d="M16 44 C15 30 17 22 22 19.5 C27 17 31 21.5 29 25 C27 28.5 22.5 26.5 23.8 23.5" fill="none" stroke="url(#br)" stroke-width="2.4" stroke-linecap="round"/>
<path d="M33 33 L37 29 L41 33 L37 37 Z" fill="url(#ros)" stroke="#1c1105" stroke-width="1.4"/>
<circle cx="11" cy="11" r="8.2" fill="#1c1105"/>
<circle cx="11" cy="11" r="6.8" fill="url(#ros)"/>
<circle cx="11" cy="11" r="3.2" fill="url(#gem)" stroke="#2a1806" stroke-width="1"/>
<circle cx="9.8" cy="9.6" r="1.1" fill="#ffffff" opacity=".85"/>'''
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}" width="{S}" height="{S}"><defs>{BRASS_DEFS}</defs>{edges}{mirror4(corner, S)}</svg>'

def frame_brass_thin():
    S = 64
    edges = f'''
<rect x="2" y="2" width="{S-4}" height="{S-4}" rx="3" fill="none" stroke="#1c1105" stroke-width="4"/>
<rect x="2" y="2" width="{S-4}" height="{S-4}" rx="3" fill="none" stroke="#c4963f" stroke-width="2"/>
<rect x="1.2" y="1.2" width="{S-2.4}" height="{S-2.4}" rx="3.6" fill="none" stroke="#f6dd98" stroke-width=".7" opacity=".8"/>'''
    corner = '''
<path d="M4 10 L10 4 L16 4 L4 16 Z" fill="url(#br)" stroke="#1c1105" stroke-width="1"/>
<circle cx="6" cy="6" r="3.4" fill="#1c1105"/>
<circle cx="6" cy="6" r="2.5" fill="url(#ros)"/>'''
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}" width="{S}" height="{S}"><defs>{BRASS_DEFS}</defs>{edges}{mirror4(corner, S)}</svg>'

def frame_brass_selected():
    # Same as thin but with a warm glow halo and gem corners: the selected build tile.
    S = 64
    edges = f'''
<rect x="2" y="2" width="{S-4}" height="{S-4}" rx="3" fill="none" stroke="#1c1105" stroke-width="4.5"/>
<rect x="2" y="2" width="{S-4}" height="{S-4}" rx="3" fill="none" stroke="#f0c86a" stroke-width="2.6"/>
<rect x="1" y="1" width="{S-2}" height="{S-2}" rx="3.8" fill="none" stroke="#fff2c4" stroke-width=".9"/>'''
    corner = '''
<path d="M3 13 L13 3 L20 3 L3 20 Z" fill="url(#br)" stroke="#1c1105" stroke-width="1"/>
<circle cx="7" cy="7" r="4.6" fill="#1c1105"/>
<circle cx="7" cy="7" r="3.6" fill="url(#ros)"/>
<circle cx="7" cy="7" r="1.7" fill="url(#gem)"/>'''
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}" width="{S}" height="{S}"><defs>{BRASS_DEFS}</defs>{edges}{mirror4(corner, S)}</svg>'

def ornament_divider():
    # A horizontal flourish for headers and between groups: line, scrolls and a centre diamond.
    W, H = 240, 24
    half = '''
<path d="M10 12 L92 12" stroke="#1c1105" stroke-width="3.2" stroke-linecap="round"/>
<path d="M10 12 L92 12" stroke="url(#br)" stroke-width="1.5" stroke-linecap="round"/>
<path d="M92 12 C98 12 101 6 106 6 C110 6 111 10 108.5 11.2 C106 12.4 104 10 106 9" fill="none" stroke="#1c1105" stroke-width="3.6" stroke-linecap="round"/>
<path d="M92 12 C98 12 101 6 106 6 C110 6 111 10 108.5 11.2 C106 12.4 104 10 106 9" fill="none" stroke="url(#br)" stroke-width="1.8" stroke-linecap="round"/>
<path d="M92 12 C98 12 101 18 106 18 C110 18 111 14 108.5 12.8" fill="none" stroke="#1c1105" stroke-width="3.6" stroke-linecap="round"/>
<path d="M92 12 C98 12 101 18 106 18 C110 18 111 14 108.5 12.8" fill="none" stroke="url(#br)" stroke-width="1.8" stroke-linecap="round"/>'''
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}"><defs>{BRASS_DEFS}</defs>'
            f'{half}<g transform="translate({W},0) scale(-1,1)">{half}</g>'
            f'<path d="M120 3 L129 12 L120 21 L111 12 Z" fill="url(#ros)" stroke="#1c1105" stroke-width="1.6"/>'
            f'<circle cx="120" cy="12" r="3" fill="url(#gem)" stroke="#2a1806" stroke-width=".8"/></svg>')

def medallion_ring():
    # A round brass bezel with notches, open in the middle, for medallion buttons (a nine-slice
    # frame whose four corners are its quarters).
    S = 96
    ticks = ''.join(f'<rect x="47" y="3.2" width="2" height="5" fill="#3a250a" transform="rotate({a} 48 48)"/>' for a in range(0, 360, 30))
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {S} {S}" width="{S}" height="{S}"><defs>{BRASS_DEFS}
<linearGradient id="rim" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff0bd"/><stop offset=".45" stop-color="#d1a14a"/><stop offset="1" stop-color="#6a4512"/></linearGradient>
<linearGradient id="rim2" x1="0" y1="1" x2="0" y2="0"><stop offset="0" stop-color="#f2d58c"/><stop offset=".5" stop-color="#a77828"/><stop offset="1" stop-color="#4e320b"/></linearGradient></defs>
<circle cx="48" cy="48" r="46" fill="#1c1105"/>
<circle cx="48" cy="48" r="44" fill="url(#rim)"/>
{ticks}
<circle cx="48" cy="48" r="38.5" fill="#1c1105"/>
<circle cx="48" cy="48" r="37" fill="url(#rim2)"/>
<circle cx="48" cy="48" r="34.2" fill="none" stroke="#1c1105" stroke-width="1.6"/>
</svg>'''

files = {
    'frame_brass.svg': frame_brass(),
    'frame_brass_thin.svg': frame_brass_thin(),
    'frame_brass_selected.svg': frame_brass_selected(),
    'ornament_divider.svg': ornament_divider(),
    'medallion_ring.svg': medallion_ring(),
}
for n, s in files.items():
    open(os.path.join(out, n), 'w').write(s)
print('wrote', len(files))
