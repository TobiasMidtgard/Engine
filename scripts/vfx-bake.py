#!/usr/bin/env python3
"""Bake six-way lit smoke flipbooks for engine_vfx.

A small offline smoke simulation (stable fluids, Stam 1999: semi-Lagrangian advection, buoyancy,
vorticity confinement, a pressure projection) whose frames are rendered from the front with a
light from each of the six axes, the way the six-way lightmaps of Unity's VFX Graph and
Battlefront II's effects are made (engine-design/particles-flipbook-research.md).

The simulation is coarse; each frame is rendered at twice its resolution with billow detail
(three octaves of Worley lumps) carried along by the flow in advected texture coordinates
(Neyret 2003) and eroding the smoke's thin edges the way Horizon Zero Dawn's clouds erode their
base shape (Schneider 2015), so the puff breaks into round, cauliflower-like lumps that move
with it. Each frame is centered on the puff and zoomed to fill its cell, like the engine's
procedural sprites fill their quad; the particle carries the motion and growth.

With an orthographic camera looking down -Z and the lights on the grid axes, the light reaching
each voxel from a direction is one cumulative sum of density along that axis, so numpy renders a
frame in a few hundred milliseconds with no ray marching.

Outputs, per sheet (an 8 x 8 grid of frames by default):
  <name>_a.png    R right, G top, B back light, A alpha
  <name>_b.png    R left, G bottom, B front light, A emission (heat)
  <name>_mv.png   RG motion of each pixel to the next frame (0.5 = still), for frame blending
  <name>.json     grid, frame count, frame rate, convention

Lightmaps hold the lit fraction (not premultiplied by alpha): a pixel's color under a white
light from +X is R_a, and so on. The tool is MIT OR Apache-2.0, like the engine.

    python3 scripts/vfx-bake.py smoke_puff --out assets/fetched/vfx/flipbooks
    python3 scripts/vfx-bake.py --list
"""

import argparse
import json
import os
import struct
import sys
import time
import zlib

import numpy as np

# ------------------------------------------------------------------------------------- sampling


def sample(field, x, y, z):
    """Trilinear sample of `field` (indexed [x, y, z]) at fractional cell coordinates."""
    n = np.array(field.shape) - 1
    x = np.clip(x, 0, n[0])
    y = np.clip(y, 0, n[1])
    z = np.clip(z, 0, n[2])
    x0 = np.minimum(np.floor(x).astype(np.int32), n[0] - 1)
    y0 = np.minimum(np.floor(y).astype(np.int32), n[1] - 1)
    z0 = np.minimum(np.floor(z).astype(np.int32), n[2] - 1)
    fx, fy, fz = x - x0, y - y0, z - z0
    out = 0.0
    for dx, wx in ((0, 1 - fx), (1, fx)):
        for dy, wy in ((0, 1 - fy), (1, fy)):
            for dz, wz in ((0, 1 - fz), (1, fz)):
                out = out + field[x0 + dx, y0 + dy, z0 + dz] * (wx * wy * wz)
    return out.astype(np.float32)


def grad(a, axis):
    return (np.roll(a, -1, axis) - np.roll(a, 1, axis)) * 0.5


def value_noise3(shape, cells, rng):
    """Smooth 3D value noise in [0, 1] with `cells` lattice cells across the grid."""
    lattice = rng.random((cells + 2,) * 3).astype(np.float32)
    axes = [np.linspace(0, cells, s, dtype=np.float32) for s in shape]
    gx, gy, gz = np.meshgrid(*axes, indexing="ij")
    return sample(lattice, gx, gy, gz)


def fbm3(shape, rng, octaves=4, base=4):
    out = np.zeros(shape, np.float32)
    amp, total = 1.0, 0.0
    for o in range(octaves):
        out += amp * value_noise3(shape, base * 2**o, rng)
        total += amp
        amp *= 0.5
    return out / total


def sample_wrap(vol, x, y, z):
    """Trilinear sample of a periodic cubic volume at any coordinates (in its own cells)."""
    s = vol.shape[0]
    x0 = np.floor(x).astype(np.int32)
    y0 = np.floor(y).astype(np.int32)
    z0 = np.floor(z).astype(np.int32)
    fx, fy, fz = (x - x0).astype(np.float32), (y - y0).astype(np.float32), (z - z0).astype(np.float32)
    x0, y0, z0 = x0 % s, y0 % s, z0 % s
    x1, y1, z1 = (x0 + 1) % s, (y0 + 1) % s, (z0 + 1) % s
    out = np.zeros(x.shape, np.float32)
    for xi, wx in ((x0, 1 - fx), (x1, fx)):
        for yi, wy in ((y0, 1 - fy), (y1, fy)):
            for zi, wz in ((z0, 1 - fz), (z1, fz)):
                out += vol[xi, yi, zi] * (wx * wy * wz)
    return out


def worley3(size, cells, rng):
    """Periodic inverted Worley noise in [0, 1]: round lumps, the cauliflower look of billowing
    smoke (the Perlin-Worley clouds of Horizon Zero Dawn, Schneider 2015)."""
    pts = rng.random((cells, cells, cells, 3)).astype(np.float32)
    i = (np.arange(size, dtype=np.float32) + 0.5) * cells / size
    gx, gy, gz = np.meshgrid(i, i, i, indexing="ij")
    cx, cy, cz = (np.floor(g).astype(np.int32) for g in (gx, gy, gz))
    best = np.full(gx.shape, 9.0, np.float32)
    for ox in (-1, 0, 1):
        for oy in (-1, 0, 1):
            for oz in (-1, 0, 1):
                nx, ny, nz = cx + ox, cy + oy, cz + oz
                p = pts[nx % cells, ny % cells, nz % cells]
                d = ((nx + p[..., 0] - gx) ** 2 + (ny + p[..., 1] - gy) ** 2
                     + (nz + p[..., 2] - gz) ** 2)
                best = np.minimum(best, d)
    return np.clip(1.0 - np.sqrt(best), 0.0, 1.0)


def billow_noise(size, rng):
    """Three octaves of Worley lumps over a little value noise, periodic, in [0, 1]."""
    out = 0.55 * worley3(size, 4, rng) + 0.3 * worley3(size, 8, rng) + 0.15 * worley3(size, 16, rng)
    lo, hi = np.percentile(out, (1, 99))
    return np.clip((out - lo) / (hi - lo), 0, 1).astype(np.float32)


# ------------------------------------------------------------------------------------ the fluid


class Smoke:
    """A cubic grid of N cells: density, temperature and velocity (cells per step)."""

    def __init__(self, n, rng):
        self.n = n
        s = (n, n, n)
        self.rho = np.zeros(s, np.float32)
        self.heat = np.zeros(s, np.float32)
        self.vel = [np.zeros(s, np.float32) for _ in range(3)]
        i = np.arange(n, dtype=np.float32)
        self.gx, self.gy, self.gz = np.meshgrid(i, i, i, indexing="ij")
        self.rng = rng
        # Two sets of texture coordinates carried by the flow, each reset to rest in turn, so
        # the detail noise rides with the smoke without stretching forever (Neyret 2003,
        # "Advected textures").
        self.uvw = [[self.gx.copy(), self.gy.copy(), self.gz.copy()] for _ in range(2)]

    def advect(self, field, dt):
        u, v, w = self.vel
        return sample(field, self.gx - u * dt, self.gy - v * dt, self.gz - w * dt)

    def project(self, iters):
        u, v, w = self.vel
        div = grad(u, 0) + grad(v, 1) + grad(w, 2)
        p = np.zeros_like(div)
        for _ in range(iters):
            p = (
                np.roll(p, 1, 0) + np.roll(p, -1, 0) + np.roll(p, 1, 1) + np.roll(p, -1, 1)
                + np.roll(p, 1, 2) + np.roll(p, -1, 2) - div
            ) / 6.0
            p[0, :, :] = p[-1, :, :] = p[:, 0, :] = p[:, -1, :] = p[:, :, 0] = p[:, :, -1] = 0.0
        for k in range(3):
            self.vel[k] -= grad(p, k)

    def confine(self, eps, dt):
        """Vorticity confinement (Fedkiw, Stam & Jensen 2001): feeds the small curls back."""
        u, v, w = self.vel
        wx = grad(w, 1) - grad(v, 2)
        wy = grad(u, 2) - grad(w, 0)
        wz = grad(v, 0) - grad(u, 1)
        mag = np.sqrt(wx * wx + wy * wy + wz * wz)
        nx, ny, nz = grad(mag, 0), grad(mag, 1), grad(mag, 2)
        nl = np.sqrt(nx * nx + ny * ny + nz * nz) + 1e-6
        nx, ny, nz = nx / nl, ny / nl, nz / nl
        self.vel[0] += eps * dt * (ny * wz - nz * wy)
        self.vel[1] += eps * dt * (nz * wx - nx * wz)
        self.vel[2] += eps * dt * (nx * wy - ny * wx)

    def step(self, dt, buoyancy, weight, cooling, fade, confinement, wind=(0, 0, 0)):
        self.vel[1] += dt * (buoyancy * self.heat - weight * self.rho)
        for k in range(3):
            self.vel[k] += dt * wind[k]
        self.confine(confinement, dt)
        self.project(30)
        self.vel = [self.advect(f, dt) for f in self.vel]
        self.project(30)
        self.rho = self.advect(self.rho, dt) * (1.0 - fade * dt)
        self.heat = self.advect(self.heat, dt) * (1.0 - cooling * dt)
        self.uvw = [[self.advect(c, dt) for c in uvw] for uvw in self.uvw]
        # Nothing leaves through the walls: fade the last cells so the puff never shows a box.
        edge = self.n // 16
        for a in range(3):
            ramp = np.minimum(np.arange(self.n), np.arange(self.n)[::-1]).astype(np.float32)
            ramp = np.clip(ramp / max(edge, 1), 0, 1)
            shape = [1, 1, 1]
            shape[a] = self.n
            self.rho *= ramp.reshape(shape)


# ---------------------------------------------------------------------------------- rendering

# Each light's direction of travel along the grid: (axis, from the high side?).
# Camera looks down -Z: +X is right, +Y is up, +Z is toward the camera (front).
LIGHTS = {
    "right": (0, True),
    "left": (0, False),
    "top": (1, True),
    "bottom": (1, False),
    "front": (2, True),
    "back": (2, False),
}


def light_reaching(rho, axis, from_high, sigma):
    """Light reaching each voxel from one side, with a few octaves of weaker extinction standing
    in for light scattered many times (Wrenninge et al., Oz), so shadow sides stay soft."""
    r = np.flip(rho, axis) if from_high else rho
    tau = np.cumsum(r, axis=axis) - r * 0.5
    if from_high:
        tau = np.flip(tau, axis)
    out = 0.0
    for k, (ext, amp) in enumerate(((1.0, 1.0), (0.35, 0.45), (0.12, 0.25))):
        out = out + amp * np.exp(-sigma * ext * tau)
    return out / 1.7


def fine_fields(smoke, up, noise, t, period, cfg):
    """The state at `up` times the simulation's resolution, with billow detail eroding the
    edges: density, heat and the two screen-plane velocities.

    The coarse density is trilinearly upsampled, then remapped by detail noise read through the
    advected texture coordinates the way Horizon Zero Dawn's clouds erode their base shape
    (Schneider 2015): thin smoke where the noise is low vanishes, dense cores stay, so the edges
    break into round lumps that move with the flow."""
    n = smoke.n
    f = np.clip((np.arange(n * up, dtype=np.float32) + 0.5) / up - 0.5, 0, n - 1)
    i0 = np.minimum(np.floor(f).astype(np.int32), n - 2)
    w1 = (f - i0).astype(np.float32)

    def up_(a):  # trilinear, one axis at a time (much cheaper than a 3D gather)
        for axis in range(3):
            shape = [1, 1, 1]
            shape[axis] = -1
            w = w1.reshape(shape)
            a = np.take(a, i0, axis) * (1.0 - w) + np.take(a, i0 + 1, axis) * w
        return a

    rho = up_(smoke.rho)
    # Two phases of advected coordinates, each weighted by a triangle that is zero when it
    # resets, blended so the noise keeps its contrast (Heitz & Neyret 2018).
    s = (t / period) % 1.0
    w = [1.0 - abs(2.0 * s - 1.0), 1.0 - abs(2.0 * ((s + 0.5) % 1.0) - 1.0)]
    freq = noise.shape[0] / cfg.get("billow_cells", 32.0)
    blend = 0.0
    for k in range(2):
        if w[k] < 1e-3:
            continue
        u, v, ww = (up_(c) * freq for c in smoke.uvw[k])
        blend = blend + w[k] * (sample_wrap(noise, u + 17.0 * k, v, ww) - 0.5)
    detail = np.clip(blend / np.sqrt(w[0] ** 2 + w[1] ** 2) + 0.5, 0, 1)
    on = rho[rho > 1e-3]
    ref = float(np.percentile(on, 99)) if on.size else 1.0
    base = np.clip(rho / ref, 0, 1)
    erode = cfg.get("erode", 0.8) * (1.0 - detail)
    shaped = np.clip((base - erode) / np.maximum(1.0 - erode, 1e-3), 0, 1)
    # Keep the dense core a little lumpy too, and give the remapped smoke back its density.
    shaped = shaped * (0.45 + 1.1 * detail) * ref
    return shaped.astype(np.float32), up_(smoke.heat), up_(smoke.vel[0]), up_(smoke.vel[1])


def render(fields, sigma):
    """Six lightmaps, alpha, emission and screen motion of the fine fields, each indexed
    [row (top first), column]."""
    rho, heat, vx, vy = fields
    # Front to back along the view (camera on the +Z side).
    tau_view = np.flip(np.cumsum(np.flip(rho, 2), axis=2), 2) - rho * 0.5
    t_view = np.exp(-sigma * tau_view)
    weight = sigma * rho * t_view  # light leaving each voxel toward the eye
    alpha = 1.0 - np.exp(-sigma * rho.sum(axis=2))
    maps = {}
    for name, (axis, high) in LIGHTS.items():
        lit = (weight * light_reaching(rho, axis, high, sigma)).sum(axis=2)
        maps[name] = lit / np.maximum(alpha, 1e-4)
    w = weight.sum(axis=2) + 1e-6
    emission = (weight * np.clip(heat, 0, None)).sum(axis=2) / w * alpha
    mx = (weight * vx).sum(axis=2) / w
    my = (weight * vy).sum(axis=2) / w

    def img(a):  # [x, y] -> [row from top, column]
        return np.flip(a.T, 0)

    return {k: img(v) for k, v in maps.items()}, img(alpha), img(emission), img(mx), -img(my)


def resample(a, cy, cx, zoom, cell, fill=0.0):
    """A cell x cell window of 2D array `a` centered on (cy, cx), `zoom` output pixels per
    input cell (bilinear)."""
    h, w = a.shape
    k = (np.arange(cell, dtype=np.float32) + 0.5 - cell / 2) / zoom
    y = cy + k
    x = cx + k
    inside = ((y >= 0) & (y <= h - 1))[:, None] & ((x >= 0) & (x <= w - 1))[None, :]
    y = np.clip(y, 0, h - 1)
    x = np.clip(x, 0, w - 1)
    y0 = np.minimum(np.floor(y).astype(int), h - 2)
    x0 = np.minimum(np.floor(x).astype(int), w - 2)
    fy = (y - y0)[:, None]
    fx = (x - x0)[None, :]
    out = (a[y0][:, x0] * (1 - fx) + a[y0][:, x0 + 1] * fx) * (1 - fy) + (
        a[y0 + 1][:, x0] * (1 - fx) + a[y0 + 1][:, x0 + 1] * fx) * fy
    return np.where(inside, out, fill).astype(np.float32)


# ---------------------------------------------------------------------------------- the sheets

PRESETS = {
    # A puff of smoke rising and spreading (campfires, chimneys, spell fumes): loops by life.
    "smoke_puff": dict(
        n=96, frames=64, substeps=2, dt=0.6, sigma=1.6, seed=7,
        source="ball", radius=0.16, height=0.3, amount=1.0, heat=1.0, inject_frames=10,
        buoyancy=0.12, weight=0.0, cooling=0.04, fade=0.006, confinement=1.5, fps=24,
    ),
    # A puff of dust kicked up by an impact or a footfall: bursts out, rolls and hangs, barely
    # rising (dust is cold), then thins as it settles.
    "dust_burst": dict(
        n=96, frames=64, substeps=2, dt=0.6, sigma=0.9, seed=11,
        source="ball", radius=0.18, height=0.45, amount=1.2, heat=0.25, inject_frames=4,
        buoyancy=0.03, weight=0.0, cooling=0.1, fade=0.008, confinement=1.2, fps=24,
        burst=0.6, swirl=0.8,
    ),
    # A rolling fireball's smoke: a hot ball that rises, rolls and cools to soot.
    "billow": dict(
        n=96, frames=64, substeps=2, dt=0.6, sigma=1.1, seed=3,
        source="ball", radius=0.2, height=0.35, amount=1.4, heat=3.0, inject_frames=6,
        buoyancy=0.05, weight=0.0, cooling=0.06, fade=0.004, confinement=0.8, fps=30,
        burst=0.5, glow=True,
    ),
}


def inject(smoke, cfg, rng, noise):
    n = smoke.n
    c = np.array([0.5, cfg["height"], 0.5]) * n
    r = cfg["radius"] * n
    dx, dy, dz = smoke.gx - c[0], smoke.gy - c[1], smoke.gz - c[2]
    if cfg["source"] == "ring":
        d = np.sqrt((np.sqrt(dx * dx + dz * dz) - r * 0.6) ** 2 + (dy * 1.6) ** 2)
        shape = np.clip(1.0 - d / (r * 0.5), 0, 1)
    else:
        d = np.sqrt(dx * dx + dy * dy + dz * dz)
        shape = np.clip(1.0 - d / r, 0, 1)
    shape = shape**0.7 * (0.5 + noise)
    smoke.rho += cfg["amount"] * 0.25 * shape
    smoke.heat += cfg["heat"] * 0.25 * shape
    # A little swirl in what comes out, so the puff breaks into billows rather than rising as
    # one smooth ball.
    swirl = cfg.get("swirl", 0.6)
    for k in range(3):
        smoke.vel[k] += swirl * shape * (np.roll(noise, 7 * (k + 1), k) - 0.5)
    burst = cfg.get("burst", 0.0)
    if burst > 0:
        dl = np.sqrt(dx * dx + dy * dy + dz * dz) + 1e-3
        for k, dd in enumerate((dx, dy, dz)):
            smoke.vel[k] += burst * shape * dd / dl


def bake(name, out_dir, cell=256, grid=8, n=None, log=print):
    cfg = dict(PRESETS[name])
    if n:
        cfg["n"] = n
    rng = np.random.default_rng(cfg["seed"])
    n = cfg["n"]
    smoke = Smoke(n, rng)
    noise = fbm3((n, n, n), rng, octaves=3, base=3)
    billows = billow_noise(64, rng)
    up = cfg.get("up", 2)
    period = cfg.get("detail_period", 24.0)  # frames between resets of each coordinate phase
    frames = min(cfg["frames"], grid * grid)
    t0 = time.time()
    shots = []
    for f in range(frames):
        for sub in range(cfg["substeps"]):
            if f < cfg["inject_frames"]:
                inject(smoke, cfg, rng, noise)
            smoke.step(cfg["dt"], cfg["buoyancy"], cfg["weight"], cfg["cooling"], cfg["fade"],
                       cfg["confinement"])
            # Keep the puff over its source: take out the mean sideways drift the swirl gives it
            # (a uniform shift of the flow, so it changes no shape).
            m = smoke.rho + 1e-6
            for k in (0, 2):
                smoke.vel[k] -= float(np.average(smoke.vel[k], weights=m))
            t = f + (sub + 1) / cfg["substeps"]
            for k in range(2):  # a phase resets where its weight crosses zero
                s = (t / period + 0.5 * k) % 1.0
                if s < 1.0 / (period * cfg["substeps"]):
                    smoke.uvw[k] = [smoke.gx.copy(), smoke.gy.copy(), smoke.gz.copy()]
        fields = fine_fields(smoke, up, billows, f + 1.0, period, cfg)
        shots.append(render(fields, cfg["sigma"] / up))
        del fields
        log(f"{name}: simulated frame {f + 1}/{frames} ({time.time() - t0:.0f} s)")

    # The camera follows the puff and zooms with it: each frame is centered on its own centroid
    # and scaled so the puff fills the cell (with a margin, so mips don't bleed across cells),
    # the way the game's procedural sprites fill their quad. The particle carries the motion
    # and the growth (its size over life); the sheet carries the shape. The zoom only ever
    # shrinks, smoothly, so the puff never seems to pulse.
    centers, extents = [], []
    for maps, alpha, *_ in shots:
        w = alpha + 1e-6
        rows, cols = np.indices(alpha.shape)
        cy, cx = (rows * w).sum() / w.sum(), (cols * w).sum() / w.sum()
        centers.append((cy, cx))
        on = alpha > 0.08
        reach = np.maximum(np.abs(rows[on] - cy), np.abs(cols[on] - cx)) if on.any() else [4.0]
        extents.append(float(np.percentile(reach, 99.0)) + 1.0)
    extents = np.maximum.accumulate(np.array(extents))
    kernel = np.ones(5) / 5.0
    padded = np.concatenate([np.full(2, extents[0]), extents, np.full(2, extents[-1])])
    extents = np.maximum.accumulate(np.convolve(padded, kernel, mode="valid"))
    margin = cell // 16
    zooms = (cell / 2 - margin) / extents  # output pixels per render pixel, per frame
    sheet_a = np.zeros((grid * cell, grid * cell, 4), np.float32)
    sheet_b = np.zeros_like(sheet_a)
    sheet_mv = np.full((grid * cell, grid * cell, 2), 0.5, np.float32)
    steps = cfg["substeps"] * cfg["dt"] * up
    k = (np.arange(cell, dtype=np.float32) + 0.5 - cell / 2)  # cell pixels from the middle
    for f, ((maps, alpha, emission, mx, my), (cy, cx)) in enumerate(zip(shots, centers)):
        zoom = zooms[f]

        def put(a, fill=0.0):
            return resample(a, cy, cx, zoom, cell, fill)

        glow = np.clip(emission / 3.0, 0, 1) if cfg.get("glow") else np.zeros_like(alpha)
        a = np.stack([put(maps["right"]), put(maps["top"]), put(maps["back"]), put(alpha)], -1)
        b = np.stack([put(maps["left"]), put(maps["bottom"]), put(maps["front"]), put(glow)], -1)
        # Where each pixel's smoke is in the next frame's cell, minus where it is now, in cell
        # UV: the flow, the camera following the centroid and the zoom.
        g = f + 1 if f + 1 < len(centers) else f
        (ny, nx), nz = centers[g], zooms[g]
        y_now = cy + k[:, None] / zoom  # render pixels under this cell's rows and columns
        x_now = cx + k[None, :] / zoom
        vx = put(mx * steps) if g != f else 0.0
        vy = put(my * steps) if g != f else 0.0
        mv_x = ((x_now + vx - nx) * nz - k[None, :]) / cell
        mv_y = ((y_now + vy - ny) * nz - k[:, None]) / cell
        mv = np.stack([np.broadcast_to(mv_x, (cell, cell)), np.broadcast_to(mv_y, (cell, cell))], -1)
        mv = mv * (a[..., 3:4] > 0.002)  # empty pixels stay still
        r, c = divmod(f, grid)
        sheet_a[r * cell:(r + 1) * cell, c * cell:(c + 1) * cell] = a
        sheet_b[r * cell:(r + 1) * cell, c * cell:(c + 1) * cell] = b
        sheet_mv[r * cell:(r + 1) * cell, c * cell:(c + 1) * cell] = np.clip(mv * 8.0 + 0.5, 0, 1)
    os.makedirs(out_dir, exist_ok=True)
    write_png(os.path.join(out_dir, f"{name}_a.png"), sheet_a)
    write_png(os.path.join(out_dir, f"{name}_b.png"), sheet_b)
    write_png(os.path.join(out_dir, f"{name}_mv.png"), sheet_mv)
    meta = {
        "grid": [grid, grid],
        "frames": frames,
        "fps": cfg["fps"],
        "cell_px": cell,
        "lightmaps": "a: right, top, back, alpha; b: left, bottom, front, emission",
        "lit_fraction": True,
        "motion_scale": 1.0 / 8.0,
        "preset": cfg,
    }
    with open(os.path.join(out_dir, f"{name}.json"), "w") as fh:
        json.dump(meta, fh, indent=2)


def write_png(path, img):
    """8-bit PNG (RGB, RGBA or gray+alpha as two channels -> RG stored as RGB with B = 0)."""
    h, w, c = img.shape
    if c == 2:
        img = np.concatenate([img, np.zeros((h, w, 1), np.float32)], -1)
        c = 3
    data = (np.clip(img, 0, 1) * 255.0 + 0.5).astype(np.uint8)
    raw = b"".join(b"\x00" + data[y].tobytes() for y in range(h))
    color = {3: 2, 4: 6}[c]

    def chunk(tag, body):
        return struct.pack(">I", len(body)) + tag + body + struct.pack(
            ">I", zlib.crc32(tag + body) & 0xFFFFFFFF)

    with open(path, "wb") as fh:
        fh.write(b"\x89PNG\r\n\x1a\n")
        fh.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, color, 0, 0, 0)))
        fh.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        fh.write(chunk(b"IEND", b""))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("names", nargs="*", help="presets to bake (default: all)")
    ap.add_argument("--out", default="assets/fetched/vfx/flipbooks")
    ap.add_argument("--cell", type=int, default=256, help="pixels per frame")
    ap.add_argument("--grid", type=int, default=8, help="frames per row and column")
    ap.add_argument("--n", type=int, help="grid cells per side (default: the preset's)")
    ap.add_argument("--list", action="store_true")
    args = ap.parse_args()
    if args.list:
        for k in PRESETS:
            print(k)
        return
    for name in args.names or list(PRESETS):
        if name not in PRESETS:
            sys.exit(f"unknown preset {name}; --list shows them")
        bake(name, args.out, args.cell, args.grid, args.n)


if __name__ == "__main__":
    main()
