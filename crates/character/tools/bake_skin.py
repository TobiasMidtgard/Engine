#!/usr/bin/env python3
"""Bake MakeHuman's CC0 default skeleton weights onto the full humanoid skeleton: the 22 body
bones, 15 finger bones per hand and the twist bones, for engine_character's `skin` module.

Usage: python3 -I bake_skin.py <makehuman checkout>/makehuman/data <out.bin>

Reads `3dobjs/base.obj`, `rigs/default.mhskel` and `rigs/default_weights.mhw` (CC0, LICENSE.ASSETS.md
of https://github.com/makehumancommunity/makehuman, commit a8bc2d54, the same as the human pack),
so the vertex order is the pack's. The file is small enough for git (crates/character/data):

  magic b"MHSK", u32 version (1)
  u32 vertex count (must equal the pack's)
  u32 bone count, then each bone name (u16 length + utf-8)
  per vertex: u8 x 4 bone indices, u8 x 4 weights (summing to 255; all 0 = no weights)
  u32 joint count, then per joint: name (u16 + utf-8), u32 n, u32 x n vertex indices
      (a bone's joint is named after the bone; a finger's tip is `<finger>_tip_<side>`)

Fingers: MakeHuman's finger1 is the thumb (finger1-1 the base of the thumb at the wrist,
finger1-2 its knuckle, finger1-3 its tip joint); finger2..5 are index to pinky, each _1 the
knuckle (MCP), _2 the middle joint (PIP), _3 the end joint (DIP). The metacarpals and the wrist
give their weights to the hand.

Twist: a limb's weight is shared between the limb bone and its twist bones by where the vertex
sits along the limb (`TWIST_NODES`: hat functions between nodes at shares of the limb's length).
engine_character turns each twist bone by a share of the limb's roll (`skin::TWIST`), so the
skin twists smoothly along the forearm instead of all at the wrist.
"""
import json
import os
import struct
import sys

UNIT = 0.1  # MakeHuman works in decimeters.
SIDES = {"L": "l", "R": "r"}
FINGERS = ["thumb", "index", "middle", "ring", "pinky"]

BODY = [
    "root", "hips", "spine", "chest", "neck", "head",
    "clavicle_l", "upper_arm_l", "lower_arm_l", "hand_l",
    "clavicle_r", "upper_arm_r", "lower_arm_r", "hand_r",
    "upper_leg_l", "lower_leg_l", "foot_l", "toe_l",
    "upper_leg_r", "lower_leg_r", "foot_r", "toe_r",
]
BONES = list(BODY)
for _s in ("l", "r"):
    for _f in FINGERS:
        for _k in (1, 2, 3):
            BONES.append("%s_0%d_%s" % (_f, _k, _s))
for _s in ("l", "r"):
    BONES += [
        "upper_arm_twist_01_" + _s, "upper_arm_twist_02_" + _s,
        "lower_arm_twist_01_" + _s, "lower_arm_twist_02_" + _s,
        "thigh_twist_01_" + _s, "calf_twist_01_" + _s,
    ]

# Per limb: (MakeHuman bones of the limb, joint at its start, joint at its end, nodes). Nodes are
# (share of the limb's length from its start, humanoid bone without the side); a vertex between
# two nodes is shared between their bones linearly, before the first node or after the last it
# goes to that node's bone. Keep in step with `skin::TWIST` (the twist shares are the roll each
# node's bone takes).
TWIST_NODES = {
    "upper_arm": (["upperarm01", "upperarm02"], "upperarm01.%s____head", "lowerarm01.%s____head",
                  [(0.10, "upper_arm_twist_01"), (0.45, "upper_arm_twist_02"), (0.85, "upper_arm")]),
    "lower_arm": (["lowerarm01", "lowerarm02"], "lowerarm01.%s____head", "wrist.%s____head",
                  [(0.10, "lower_arm"), (0.45, "lower_arm_twist_01"), (0.85, "lower_arm_twist_02")]),
    "upper_leg": (["upperleg01", "upperleg02"], "upperleg01.%s____head", "lowerleg01.%s____head",
                  [(0.15, "thigh_twist_01"), (0.60, "upper_leg")]),
    "lower_leg": (["lowerleg01", "lowerleg02"], "lowerleg01.%s____head", "foot.%s____head",
                  [(0.20, "lower_leg"), (0.75, "calf_twist_01")]),
}


def split(mh):
    side = None
    base = mh
    if mh.endswith(".L") or mh.endswith(".R"):
        side = SIDES[mh[-1]]
        base = mh[:-2]
    return base, side


def humanoid_bones(mh):
    """The humanoid bones (with shares) a MakeHuman bone's weights go to; a limb returns its
    `TWIST_NODES` key instead of a bone (shared by position later)."""
    base, side = split(mh)
    if base in ("root", "spine05", "pelvis"):
        return [("hips", 1.0)]
    if base in ("spine04", "spine03"):
        return [("spine", 1.0)]
    if base in ("spine02", "spine01", "breast"):
        return [("chest", 1.0)]
    if base.startswith("neck"):
        return [("neck", 1.0)]
    if side is None:
        return [("head", 1.0)]
    if base == "clavicle":
        return [("clavicle_" + side, 1.0)]
    if base == "shoulder01":
        return [("clavicle_" + side, 0.5), ("upper_arm_" + side, 0.5)]
    for limb, (parts, _, _, _) in TWIST_NODES.items():
        if base in parts:
            return [("limb:" + limb + ":" + side, 1.0)]
    if base == "wrist" or base.startswith("metacarpal"):
        return [("hand_" + side, 1.0)]
    if base.startswith("finger"):
        f, k = base[len("finger"):].split("-")
        return [("%s_0%s_%s" % (FINGERS[int(f) - 1], k, side), 1.0)]
    if base == "foot":
        return [("foot_" + side, 1.0)]
    if base.startswith("toe"):
        return [("toe_" + side, 1.0)]
    return [("head", 1.0)]


def load_verts(path):
    verts = []
    groups = {}
    group = None
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.startswith("v "):
                x, y, z = map(float, line.split()[1:4])
                verts.append((x * UNIT, y * UNIT, z * UNIT))
            elif line.startswith("g "):
                group = line.split(None, 1)[1].strip()
            elif line.startswith("f ") and group is not None:
                for p in line.split()[1:]:
                    groups.setdefault(group, set()).add(int(p.split("/")[0]) - 1)
    return verts, groups


def mean(verts, idx):
    n = max(len(idx), 1)
    return tuple(sum(verts[i][c] for i in idx) / n for c in range(3))


def along(p, a, b):
    d = [b[c] - a[c] for c in range(3)]
    dd = sum(x * x for x in d) or 1e-9
    return sum((p[c] - a[c]) * d[c] for c in range(3)) / dd


def hat(t, nodes):
    """Shares of the nodes' bones at t along the limb."""
    if t <= nodes[0][0]:
        return [(nodes[0][1], 1.0)]
    for (t0, b0), (t1, b1) in zip(nodes, nodes[1:]):
        if t <= t1:
            u = (t - t0) / (t1 - t0)
            return [(b0, 1.0 - u), (b1, u)]
    return [(nodes[-1][1], 1.0)]


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
    for _i, _f in enumerate(FINGERS):
        for _k in (1, 2, 3):
            JOINTS["%s_0%d_%s" % (_f, _k, _s)] = ["finger%d-%d.%s____head" % (_i + 1, _k, _mh)]
        JOINTS["%s_tip_%s" % (_f, _s)] = ["finger%d-3.%s____tail" % (_i + 1, _mh)]


def write_str(out, s):
    b = s.encode("utf-8")
    out.write(struct.pack("<H", len(b)))
    out.write(b)


def main():
    data, out_path = sys.argv[1], sys.argv[2]
    verts, groups = load_verts(os.path.join(data, "3dobjs", "base.obj"))
    skel = json.load(open(os.path.join(data, "rigs", "default.mhskel"), encoding="utf-8"))
    weights = json.load(open(os.path.join(data, "rigs", "default_weights.mhw"), encoding="utf-8"))
    index = {b: i for i, b in enumerate(BONES)}
    nv = len(verts)
    limb_ends = {}
    for limb, (_, start, end, _) in TWIST_NODES.items():
        for mh, s in SIDES.items():
            a = mean(verts, skel["joints"][start % mh])
            b = mean(verts, skel["joints"][end % mh])
            limb_ends[(limb, s)] = (a, b)
    per_vertex = [dict() for _ in range(nv)]
    for mh, pairs in weights["weights"].items():
        for bone, share in humanoid_bones(mh):
            for v, w in pairs:
                if v >= nv:
                    continue
                if bone.startswith("limb:"):
                    _, limb, s = bone.split(":")
                    a, b = limb_ends[(limb, s)]
                    t = along(verts[v], a, b)
                    for node, u in hat(t, TWIST_NODES[limb][3]):
                        k = index[node + "_" + s]
                        per_vertex[v][k] = per_vertex[v].get(k, 0.0) + w * share * u
                else:
                    k = index[bone]
                    per_vertex[v][k] = per_vertex[v].get(k, 0.0) + w * share
    packed = []
    for d in per_vertex:
        top = sorted(((b, w) for b, w in d.items() if w > 0), key=lambda kv: -kv[1])[:4]
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
    for name, refs in JOINTS.items():
        if refs is None:
            vs = sorted(groups["joint-ground"])
        else:
            vs = sorted({v for r in refs for v in skel["joints"][r]})
        joints.append((name, vs))
    with open(out_path, "wb") as out:
        out.write(b"MHSK")
        out.write(struct.pack("<I", 1))
        out.write(struct.pack("<I", nv))
        out.write(struct.pack("<I", len(BONES)))
        for b in BONES:
            write_str(out, b)
        for bones, w in packed:
            out.write(struct.pack("<4B4B", *bones, *w))
        out.write(struct.pack("<I", len(joints)))
        for name, vs in joints:
            write_str(out, name)
            out.write(struct.pack("<I", len(vs)))
            out.write(struct.pack("<%dI" % len(vs), *vs))
    print("verts", nv, "bones", len(BONES), "joints", len(joints), "bytes", os.path.getsize(out_path))


if __name__ == "__main__":
    main()
