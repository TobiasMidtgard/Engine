#!/usr/bin/env python3
"""Bake MakeHuman's CC0 base mesh (hm08) and shape targets into one pack for engine_character.

Usage: python3 -I bake_makehuman.py <makehuman checkout>/makehuman/data <out.bin>

The MakeHuman assets (base mesh and targets) were released as CC0 in September 2020
(LICENSE.ASSETS.md of https://github.com/makehumancommunity/makehuman). The pack holds:

  magic b"MHPK", u32 version (1)
  u32 vertex count, then f32 x,y,z per vertex (meters, Y up, the person facing +Z)
  u32 uv count, then f32 u,v per uv
  u32 group count, then each group name (u16 length + utf-8)
  u32 quad count, then per quad: u32 x4 vertex indices, u32 x4 uv indices, u16 group
  u32 target count, then per target:
      name (u16 + utf-8), u8 factor count, factor names (u16 + utf-8 each),
      f32 scale, u32 n, u32 x n vertex indices, i16 x 3 x n deltas (delta = i16 * scale, m)

A target's weight is the product of its factors (see engine_character's README): macro
values such as `female`, `young`, `maxweight`, `african`, and slider halves such as
`nose-hump-incr`.
"""
import os
import struct
import sys

CATEGORIES = [
    ("gender", ["male", "female"]),
    ("age", ["baby", "child", "young", "old"]),
    ("race", ["caucasian", "asian", "african"]),
    ("muscle", ["maxmuscle", "averagemuscle", "minmuscle"]),
    ("weight", ["minweight", "averageweight", "maxweight"]),
    ("height", ["minheight", "averageheight", "maxheight"]),
    ("breastsize", ["mincup", "averagecup", "maxcup"]),
    ("breastfirmness", ["minfirmness", "averagefirmness", "maxfirmness"]),
    ("bodyproportions", ["uncommonproportions", "regularproportions", "idealproportions"]),
]
MACRO_VALUES = {v for _, vs in CATEGORIES for v in vs}

# Target folders baked (relative to data/targets). Macro folders shape the whole body; the
# rest are the face and neck sliders plus the torso and breast for the bust.
FOLDERS = [
    "macrodetails", "head", "forehead", "eyebrows", "neck", "eyes", "nose", "mouth",
    "ears", "chin", "cheek", "torso", "breast",
]
UNIT = 0.1  # MakeHuman works in decimeters.


def write_str(out, s):
    b = s.encode("utf-8")
    out.write(struct.pack("<H", len(b)))
    out.write(b)


def load_obj(path):
    verts, uvs, quads, groups = [], [], [], []
    group_index = {}
    group = 0
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.startswith("v "):
                x, y, z = map(float, line.split()[1:4])
                verts.append((x * UNIT, y * UNIT, z * UNIT))
            elif line.startswith("vt "):
                u, v = map(float, line.split()[1:3])
                uvs.append((u, v))
            elif line.startswith("g "):
                name = line.split(None, 1)[1].strip()
                if name not in group_index:
                    group_index[name] = len(groups)
                    groups.append(name)
                group = group_index[name]
            elif line.startswith("f "):
                parts = line.split()[1:]
                vi = [int(p.split("/")[0]) - 1 for p in parts]
                ti = [int(p.split("/")[1]) - 1 for p in parts]
                if len(vi) == 3:
                    vi.append(vi[2])
                    ti.append(ti[2])
                assert len(vi) == 4, line
                quads.append((vi, ti, group))
    return verts, uvs, quads, groups


def target_factors(rel):
    """Factors of a target from its path, as MakeHuman's targets.Component does it."""
    parts = []
    for comp in rel[:-len(".target")].replace("\\", "/").split("/"):
        parts.extend(comp.split("-"))
    macro = [p for p in parts if p in MACRO_VALUES]
    return macro


def load_target(path):
    idx, deltas = [], []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            p = line.split()
            idx.append(int(p[0]))
            deltas.append(tuple(float(c) * UNIT for c in p[1:4]))
    return idx, deltas


def main():
    data, out_path = sys.argv[1], sys.argv[2]
    verts, uvs, quads, groups = load_obj(os.path.join(data, "3dobjs", "base.obj"))
    targets = []
    for folder in FOLDERS:
        root = os.path.join(data, "targets", folder)
        for dirpath, _, files in os.walk(root):
            for name in sorted(files):
                if not name.endswith(".target"):
                    continue
                path = os.path.join(dirpath, name)
                rel = os.path.relpath(path, os.path.join(data, "targets"))
                idx, deltas = load_target(path)
                if not idx:
                    continue
                macro = target_factors(rel)
                tname = rel[:-len(".target")].replace("\\", "/")
                if folder == "macrodetails":
                    factors = macro
                else:
                    # Slider half: the file name without macro parts, e.g. nose-hump-incr.
                    stem = "-".join(p for p in name[:-len(".target")].split("-") if p not in MACRO_VALUES)
                    factors = ([stem] if stem else []) + macro
                targets.append((tname, factors, idx, deltas))
    with open(out_path, "wb") as out:
        out.write(b"MHPK")
        out.write(struct.pack("<I", 1))
        out.write(struct.pack("<I", len(verts)))
        for v in verts:
            out.write(struct.pack("<3f", *v))
        out.write(struct.pack("<I", len(uvs)))
        for t in uvs:
            out.write(struct.pack("<2f", *t))
        out.write(struct.pack("<I", len(groups)))
        for g in groups:
            write_str(out, g)
        out.write(struct.pack("<I", len(quads)))
        for vi, ti, g in quads:
            out.write(struct.pack("<4I4IH", *vi, *ti, g))
        out.write(struct.pack("<I", len(targets)))
        for tname, factors, idx, deltas in sorted(targets):
            write_str(out, tname)
            out.write(struct.pack("<B", len(factors)))
            for fct in factors:
                write_str(out, fct)
            peak = max(max(abs(c) for c in d) for d in deltas) or 1e-6
            scale = peak / 32767.0
            out.write(struct.pack("<f", scale))
            out.write(struct.pack("<I", len(idx)))
            out.write(struct.pack("<%dI" % len(idx), *idx))
            q = []
            for d in deltas:
                q.extend(int(round(c / scale)) for c in d)
            out.write(struct.pack("<%dh" % len(q), *q))
    print("verts", len(verts), "uvs", len(uvs), "quads", len(quads), "groups", len(groups),
          "targets", len(targets), "bytes", os.path.getsize(out_path))


if __name__ == "__main__":
    main()
