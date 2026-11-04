"""Round 25 step c: crops of the night pictures (png/r25/*.png, 1600x900): the water near the shore in h2_5_lake (where
round 24 had the green wedge; 1x overview, then 4-6x), and the Moon (night24.py's predicted pixel) at 5-6x.

python -I crops25.py <run dir> [moon_x moon_y]   (the Moon's predicted pixel in h3_5_sunset; default round 24's (914, 19))
Writes <run dir>/crops/*.png and prints each crop's box, its brightest pixel and its greenest pixel (g - (r + b) / 2).
"""
import os, sys
from PIL import Image

d = sys.argv[1]
mx, my = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) > 3 else (914, 19)


def crop(src, x0, y0, x1, y1, scale, out, note):
    im = Image.open(f"{d}/png/r25/{src}.png").convert("RGB")
    x0, y0, x1, y1 = max(0, x0), max(0, y0), min(im.width, x1), min(im.height, y1)
    c = im.crop((x0, y0, x1, y1))
    c.resize(((x1 - x0) * scale, (y1 - y0) * scale), Image.NEAREST).save(f"{d}/crops/{out}.png")
    px = c.load()
    pts = [(px[i, j], i + x0, j + y0) for i in range(x1 - x0) for j in range(y1 - y0)]
    best = max(pts, key=lambda p: sum(p[0]))
    green = max(pts, key=lambda p: p[0][1] - (p[0][0] + p[0][2]) / 2)
    mean = [sum(p[0][k] for p in pts) / len(pts) for k in range(3)]
    print(f"{out}: {src} box ({x0},{y0})-({x1},{y1}) x{scale}; brightest pixel ({best[1]}, {best[2]}) rgb {best[0]}; "
          f"greenest ({green[1]}, {green[2]}) rgb {green[0]}; mean rgb ({mean[0]:.0f}, {mean[1]:.0f}, {mean[2]:.0f}); {note}")


def around(src, cx, cy, w, h, scale, out, note):
    crop(src, int(cx - w / 2), int(cy - h / 2), int(cx + w / 2), int(cy + h / 2), scale, out, note)


os.makedirs(f"{d}/crops", exist_ok=True)
crop("h2_5_lake", 0, 720, 960, 900, 1, "near_water_h2_5_lake_1x", "round 24's green-wedge box (bottom left), 1x")
crop("h2_5_lake", 0, 640, 1600, 900, 1, "near_shore_h2_5_lake_1x", "the whole near water and shore, 1x")
crop("h2_5_lake", 60, 760, 300, 900, 4, "near_water_left_h2_5_lake_x4", "where the wedge's wide end was")
crop("h2_5_lake", 420, 730, 660, 870, 4, "near_water_mid_h2_5_lake_x4", "the wedge's middle")
crop("h2_5_lake", 760, 700, 1000, 840, 4, "near_water_tip_h2_5_lake_x4", "the wedge's tip (round 24 ~(960, 750))")
crop("h2_5_lake", 1000, 780, 1240, 900, 4, "near_shore_right_h2_5_lake_x4", "the near beach and the water's edge, right")
crop("h2_5_lake", 1340, 760, 1600, 880, 4, "near_shore_far_right_h2_5_lake_x4", "the near beach, far right")
crop("h2_5_lake", 260, 400, 420, 470, 6, "far_shore_waterfall_h2_5_lake_x6", "the far shore under the waterfall")
around("h3_5_sunset", mx, my, 96, 64, 6, "moon_h3_5_sunset_x6", f"the space Moon, predicted ({mx}, {my})")
around("h3_5_sunset", mx, my, 40, 30, 12, "moon_h3_5_sunset_x12", f"the space Moon, closer")
