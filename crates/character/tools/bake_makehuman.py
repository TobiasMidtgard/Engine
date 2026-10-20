#!/usr/bin/env python3
"""Bake MakeHuman's CC0 base mesh (hm08) and shape targets into one pack for engine_character.

Usage: python3 -I bake_makehuman.py <makehuman checkout>/makehuman/data <out.bin>

The MakeHuman assets (base mesh, targets, the default skeleton and its weights) were released
as CC0 (LICENSE.ASSETS.md of https://github.com/makehumancommunity/makehuman). The pack holds:

  magic b"MHPK", u32 version (2)
  u32 vertex count, then f32 x,y,z per vertex (meters, Y up, the person facing +Z)
  u32 uv count, then f32 u,v per uv
  u32 group count, then each group name (u16 length + utf-8)
  u32 quad count, then per quad: u32 x4 vertex indices, u32 x4 uv indices, u16 group
  u32 target count, then per target:
      name (u16 + utf-8), u8 factor count, factor names (u16 + utf-8 each),
      f32 scale, u32 n, u32 x n vertex indices, i16 x 3 x n deltas (delta = i16 * scale, m)
  u32 bone count, then each bone name (u16 + utf-8): the animation module's humanoid bones
  per vertex: u8 x 4 bone indices, u8 x 4 weights (summing to 255; all 0 = no weights)
  u32 joint count, then per joint: bone name (u16 + utf-8), u32 n, u32 x n vertex indices
      (the joint sits at the mean of those vertices, so it follows the morphs)

A target's weight is the product of its factors (see engine_character's README): macro
values such as `female`, `young`, `maxweight`, `african`, and slider halves such as
`nose-hump-incr`. Expression units (`expression/units/<ancestry>/mouth-open`) get the factor
`expr-mouth-open` and their ancestry, so they stay off until a face is animated.

Skin weights: MakeHuman's default skeleton (163 bones) is folded onto the animation module's
27-bone humanoid (see `HUMANOID`): every MakeHuman bone gives its weights to the humanoid bone
it belongs to (fingers to the hand, face bones to the head), keeping the four largest.
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
    "ears", "chin", "cheek", "torso", "breast", "expression",
]
UNIT = 0.1  # MakeHuman works in decimeters.

# The animation module's humanoid bones, in pack order.
HUMANOID = [
    "root", "hips", "spine", "chest", "neck", "head",
    "clavicle_l", "upper_arm_l", "lower_arm_l", "hand_l",
    "clavicle_r", "upper_arm_r", "lower_arm_r", "hand_r",
    "upper_leg_l", "lower_leg_l", "foot_l", "toe_l",
    "upper_leg_r", "lower_leg_r", "foot_r", "toe_r",
]
SIDES = {"L": "l", "R": "r"}


def humanoid_bones(mh):
    """The humanoid bones (with shares) a MakeHuman bone's weights go to."""
    side = None
    base = mh
    if mh.endswith(".L") or mh.endswith(".R"):
        side = SIDES[mh[-1]]
        base = mh[:-2]
    if base in ("root", "spine05", "pelvis"):
        return [("hips", 1.0)]
    if base in ("spine04", "spine03"):
        return [("spine", 1.0)]
    if base in ("spine02", "spine01", "breast"):
        return [("chest", 1.0)]
    if base.startswith("neck"):
        return [("neck", 1.0)]
    if side is None:
        return [("head", 1.0)]  # head, jaw, mouth, tongue, ...
    if base == "clavicle":
        return [("clavicle_" + side, 1.0)]
    if base == "shoulder01":
        # The deltoid: halfway between the shoulder blade and the arm.
        return [("clavicle_" + side, 0.5), ("upper_arm_" + side, 0.5)]
    if base.startswith("upperarm"):
        return [("upper_arm_" + side, 1.0)]
    if base.startswith("lowerarm"):
        return [("lower_arm_" + side, 1.0)]
    if base in ("wrist",) or base.startswith("metacarpal") or base.startswith("finger"):
        return [("hand_" + side, 1.0)]
    if base.startswith("upperleg"):
        return [("upper_leg_" + side, 1.0)]
    if base.startswith("lowerleg"):
        return [("lower_leg_" + side, 1.0)]
    if base == "foot":
        return [("foot_" + side, 1.0)]
    if base.startswith("toe"):
        return [("toe_" + side, 1.0)]
    return [("head", 1.0)]  # eyes, lids, brows, cheeks, lips


# Where each humanoid joint sits: MakeHuman skeleton joints whose vertices it averages
# (`None` = the base mesh's `joint-ground` group).
JOINTS = {
    "root": None,
    "hips": ["spine05____head"],
    "spine": ["spine03____head"],
    "chest": ["spine02____head"],
    "neck": ["neck01____head"],
    "head": ["head____head"],
}
for _mh, _s in SIDES.items():
    JOINTS.update({
        "clavicle_" + _s: ["clavicle.%s____head" % _mh],
        "upper_arm_" + _s: ["upperarm01.%s____head" % _mh],
        "lower_arm_" + _s: ["lowerarm01.%s____head" % _mh],
        "hand_" + _s: ["wrist.%s____head" % _mh],
        "upper_leg_" + _s: ["upperleg01.%s____head" % _mh],
        "lower_leg_" + _s: ["lowerleg01.%s____head" % _mh],
        "foot_" + _s: ["foot.%s____head" % _mh],
        "toe_" + _s: ["toe%d-1.%s____head" % (k, _mh) for k in range(1, 6)],
    })


def load_rig(data, nverts, groups_verts):
    import json
    skel = json.load(open(os.path.join(data, "rigs", "default.mhskel"), encoding="utf-8"))
    weights = json.load(open(os.path.join(data, "rigs", "default_weights.mhw"), encoding="utf-8"))
    index = {b: i for i, b in enumerate(HUMANOID)}
    per_vertex = [dict() for _ in range(nverts)]
    for mh, pairs in weights["weights"].items():
        for bone, share in humanoid_bones(mh):
            b = index[bone]
            for v, w in pairs:
                if v < nverts:
                    per_vertex[v][b] = per_vertex[v].get(b, 0.0) + w * share
    packed = []
    for d in per_vertex:
        top = sorted(d.items(), key=lambda kv: -kv[1])[:4]
        total = sum(w for _, w in top)
        if total <= 0:
            packed.append(([0, 0, 0, 0], [0, 0, 0, 0]))
            continue
        q = [int(round(255 * w / total)) for _, w in top]
        q[0] += 255 - sum(q)
        bones = [b for b, _ in top]
        while len(bones) < 4:
            bones.append(0)
            q.append(0)
        packed.append((bones, q))
    joints = []
    for name in HUMANOID:
        refs = JOINTS[name]
        if refs is None:
            vs = groups_verts["joint-ground"]
        else:
            vs = sorted({v for r in refs for v in skel["joints"][r]})
        joints.append((name, vs))
    return packed, joints


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
    groups_verts = {}
    for vi, _, g in quads:
        groups_verts.setdefault(groups[g], set()).update(vi)
    skin, joints = load_rig(data, len(verts), groups_verts)
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
                elif folder == "expression":
                    stem = name[:-len(".target")]
                    factors = ["expr-" + stem] + macro
                else:
                    # Slider half: the file name without macro parts, e.g. nose-hump-incr.
                    stem = "-".join(p for p in name[:-len(".target")].split("-") if p not in MACRO_VALUES)
                    factors = ([stem] if stem else []) + macro
                targets.append((tname, factors, idx, deltas))
    with open(out_path, "wb") as out:
        out.write(b"MHPK")
        out.write(struct.pack("<I", 2))
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
        out.write(struct.pack("<I", len(HUMANOID)))
        for b in HUMANOID:
            write_str(out, b)
        for bones, w in skin:
            out.write(struct.pack("<4B4B", *bones, *w))
        out.write(struct.pack("<I", len(joints)))
        for name, vs in joints:
            write_str(out, name)
            out.write(struct.pack("<I", len(vs)))
            out.write(struct.pack("<%dI" % len(vs), *vs))
    print("verts", len(verts), "uvs", len(uvs), "quads", len(quads), "groups", len(groups),
          "targets", len(targets), "bytes", os.path.getsize(out_path))


if __name__ == "__main__":
    main()
