"""Round 24 step c: 4-5x crops of the night pictures (png/r24/*.png, 1600x900) around each predicted moon
(night24.py), the spots where round 23's weather moon stood, and the green wedge on the water in h2_5_lake.

python -I crops24.py <run dir>   (run from anywhere; writes <run dir>/crops/*.png and prints each crop's brightest pixel)
"""
import os, sys
from PIL import Image

d = sys.argv[1]


def crop(src, cx, cy, w, h, scale, out, note):
    im = Image.open(f"{d}/png/r24/{src}.png").convert("RGB")
    x0, y0 = max(0, int(cx - w / 2)), max(0, int(cy - h / 2))
    x1, y1 = min(im.width, x0 + w), min(im.height, y0 + h)
    c = im.crop((x0, y0, x1, y1)).resize(((x1 - x0) * scale, (y1 - y0) * scale), Image.NEAREST)
    c.save(f"{d}/crops/{out}.png")
    px = im.crop((x0, y0, x1, y1)).load()
    best = max((sum(px[i, j]), i + x0, j + y0) for i in range(x1 - x0) for j in range(y1 - y0))
    print(f"{out}: {src} box ({x0},{y0})-({x1},{y1}) x{scale}; brightest pixel {best[1:]} rgb-sum {best[0]}; {note}")


os.makedirs(f"{d}/crops", exist_ok=True)
crop("h3_5_sunset", 914, 19, 96, 64, 5, "space_moon_h3_5_sunset", "the space Moon, predicted (914, 19)")
crop("h3_5_sunset", 1067, 341, 96, 64, 5, "old_clock_moon_spot_h3_5_sunset", "where round 23's weather moon stood (behind the tree)")
crop("h2_5_lake", 1196, 61, 96, 64, 5, "old_clock_moon_spot_h2_5_lake",
     "where round 23's full disc was; the Moon is now above the frame at (991, -305)")
crop("h2_5_lake", 991, 32, 160, 64, 4, "top_edge_below_moon_h2_5_lake", "the top edge under the space Moon (above the frame)")
crop("h3_5_lake", 1190, 32, 160, 64, 4, "top_edge_below_moon_h3_5_lake", "the top edge under the space Moon, predicted (1190, -89)")
crop("h2_5_lake", 480, 810, 960, 180, 1, "green_slab_h2_5_lake", "the bright green wedge on the water, bottom left (not a moon)")
