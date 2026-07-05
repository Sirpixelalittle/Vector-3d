#!/usr/bin/env python3
"""Generate the arena game assets (assets/arena/): a big open octagonal
fight pit with a Battlezone mountain horizon, plus the two enemy models.

Enemies face the player with their local −Z, so the sentinel's floating
"eye" (a diamond hovering just off the body — occluded when it looks away)
tells you exactly who it is hunting.

Run from the repo root:  python3 tools/gen_arena.py
"""

import math
import random
from pathlib import Path

from gltf_builder import GltfBuilder, LINES, LINE_LOOP, LINE_STRIP, TRIANGLES

OUT_DIR = Path(__file__).resolve().parent.parent / "assets" / "arena"

WALL_R = 26.0     # octagon apothem-ish radius (vertex radius)
WALL_H = 3.5
FLOOR_HALF = 34.0
GATE_ANGLES = (0.0, 90.0, 180.0, 270.0)  # spawn gates, degrees


def octagon(radius, y=0.0, offset_deg=22.5):
    return [
        (
            radius * math.cos(math.radians(offset_deg + 45.0 * i)),
            y,
            radius * math.sin(math.radians(offset_deg + 45.0 * i)),
        )
        for i in range(8)
    ]


def quad(points, a, b, c, d):
    base = len(points)
    points.extend([a, b, c, d])
    return [base, base + 1, base + 2, base, base + 2, base + 3]


def box(points, cx, cz, half, height):
    tris = []
    corners = [
        (cx - half, cz - half), (cx + half, cz - half),
        (cx + half, cz + half), (cx - half, cz + half),
    ]
    for i in range(4):
        (x0, z0), (x1, z1) = corners[i], corners[(i + 1) % 4]
        tris += quad(points, (x0, 0, z0), (x1, 0, z1), (x1, height, z1), (x0, height, z0))
    tris += quad(
        points,
        (cx - half, height, cz - half), (cx + half, height, cz - half),
        (cx + half, height, cz + half), (cx - half, height, cz + half),
    )
    return tris


def spiral(cx, cz, extent, step=0.3, y=0.0):
    points = [(cx - extent, y, cz - extent)]
    x, z = cx - extent, cz - extent
    length = 2 * extent
    directions = [(1, 0), (0, 1), (-1, 0), (0, -1)]
    leg = 0
    while length > 0.2:
        dx, dz = directions[leg % 4]
        x += dx * length
        z += dz * length
        points.append((x, y, z))
        if leg % 2 == 1:
            length -= step
        leg += 1
    return points


def make_arena():
    g = GltfBuilder()
    structure = g.material("structure", base_color=[0.30, 0.85, 0.12])
    panel = g.material("panel", emissive=[0.55, 1.0, 0.10])
    floor_art = g.material("floor-art", emissive=[0.05, 1.0, 0.15], emissive_strength=1.6)
    gate = g.material("gate-dash", emissive=[0.05, 0.75, 1.0], emissive_strength=2.0)
    horizon = g.material("horizon", emissive=[0.25, 0.75, 0.20])

    pts, tris = [], []
    # Floor + octagon walls.
    tris += quad(
        pts,
        (-FLOOR_HALF, 0, -FLOOR_HALF), (FLOOR_HALF, 0, -FLOOR_HALF),
        (FLOOR_HALF, 0, FLOOR_HALF), (-FLOOR_HALF, 0, FLOOR_HALF),
    )
    ring = octagon(WALL_R)
    for i in range(8):
        (x0, _, z0), (x1, _, z1) = ring[i], ring[(i + 1) % 8]
        tris += quad(pts, (x0, 0, z0), (x1, 0, z1), (x1, WALL_H, z1), (x0, WALL_H, z0))
    # Four pillars for cover and occlusion drama.
    for angle in (45.0, 135.0, 225.0, 315.0):
        cx = 10.0 * math.cos(math.radians(angle))
        cz = 10.0 * math.sin(math.radians(angle))
        tris += box(pts, cx, cz, 0.8, 3.0)

    primitives = [g.primitive(pts, tris, structure)]

    def loop(loop_points, material):
        primitives.append(
            g.primitive(loop_points, list(range(len(loop_points))), material, mode=LINE_LOOP)
        )

    def strip(strip_points, material):
        primitives.append(
            g.primitive(strip_points, list(range(len(strip_points))), material, mode=LINE_STRIP)
        )

    # Floor art: concentric octagons + center spiral + gate ticks.
    for radius, mat in ((6.0, floor_art), (12.0, floor_art), (18.0, panel), (24.0, panel)):
        loop(octagon(radius, y=0.0), mat)
    strip(spiral(0.0, 0.0, 2.6), floor_art)

    # Spawn gates: dashed arches on the walls + floor ticks pointing in.
    for angle in GATE_ANGLES:
        rad = math.radians(angle)
        cx, cz = WALL_R * math.cos(rad), WALL_R * math.sin(rad)
        # Tangent along the wall, pointing "left" when facing the center.
        tx, tz = -math.sin(rad), math.cos(rad)
        # Pull decor slightly inward so it sits proud of the wall plane.
        ix, iz = cx * 0.995, cz * 0.995
        arch = [
            (ix - tx * 1.4, 0.2, iz - tz * 1.4),
            (ix - tx * 1.4, 2.8, iz - tz * 1.4),
            (ix + tx * 1.4, 2.8, iz + tz * 1.4),
            (ix + tx * 1.4, 0.2, iz + tz * 1.4),
        ]
        strip(arch, gate)
        tick_pts, tick_idx = [], []
        for k in (0.86, 0.90, 0.94):
            tick_idx += [len(tick_pts), len(tick_pts) + 1]
            tick_pts += [
                (cx * k - tx * 0.4, 0.0, cz * k - tz * 0.4),
                (cx * k + tx * 0.4, 0.0, cz * k + tz * 0.4),
            ]
        primitives.append(g.primitive(tick_pts, tick_idx, gate, mode=LINES))

    # Battlezone horizon: a jagged mountain ring far outside the walls.
    random.seed(7)
    mountains = []
    n = 64
    for i in range(n + 1):
        a = 2 * math.pi * (i % n) / n
        height = 2.0 + random.random() * 5.5 if i % 2 else 0.6
        mountains.append((55.0 * math.cos(a), height, 55.0 * math.sin(a)))
    strip(mountains, horizon)

    g.mesh("arena", primitives)
    g.write(OUT_DIR / "arena.gltf")


def make_shard():
    """Fast, fragile crystal: a hexagonal bipyramid, all crease edges."""
    g = GltfBuilder()
    body = g.material("shard", emissive=[1.0, 0.15, 0.9], emissive_strength=1.3)
    pts = [(0.0, 0.55, 0.0), (0.0, -0.55, 0.0)]
    ring = [
        (0.32 * math.cos(math.pi / 3 * i), 0.0, 0.32 * math.sin(math.pi / 3 * i))
        for i in range(6)
    ]
    pts += ring
    tris = []
    for i in range(6):
        j = (i + 1) % 6
        tris += [0, 2 + i, 2 + j]  # upper fan
        tris += [1, 2 + j, 2 + i]  # lower fan
    g.mesh("shard", [g.primitive(pts, tris, body)])
    g.write(OUT_DIR / "shard.gltf")


def make_sentinel():
    """Slow, tanky octahedron with a floating eye on its −Z face — the eye
    is only visible when it is looking at you."""
    g = GltfBuilder()
    body = g.material("sentinel", base_color=[1.0, 0.55, 0.05])
    eye = g.material("sentinel-eye", emissive=[1.0, 0.25, 0.05], emissive_strength=2.2)
    pts = [(0.0, 0.9, 0.0), (0.0, -0.9, 0.0)]
    ring = [(0.62, 0.0, 0.0), (0.0, 0.0, 0.62), (-0.62, 0.0, 0.0), (0.0, 0.0, -0.62)]
    pts += ring
    tris = []
    for i in range(4):
        j = (i + 1) % 4
        tris += [0, 2 + i, 2 + j]
        tris += [1, 2 + j, 2 + i]
    primitives = [g.primitive(pts, tris, body)]
    # Floating iris just off the front face.
    r, z = 0.13, -0.72
    iris = [(r, 0.0, z), (0.0, r, z), (-r, 0.0, z), (0.0, -r, z)]
    primitives.append(g.primitive(iris, [0, 1, 2, 3], eye, mode=LINE_LOOP))
    g.mesh("sentinel", primitives)
    g.write(OUT_DIR / "sentinel.gltf")


if __name__ == "__main__":
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    make_arena()
    make_shard()
    make_sentinel()
