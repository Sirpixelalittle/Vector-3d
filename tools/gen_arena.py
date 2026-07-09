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




def make_healthpack():
    """Retro medkit: a glowing red cross prism. Built lazily from five
    boxes of duplicated quads -- the converter's welding merges the seams
    and its coplanar-edge drop erases the internal borders, leaving only
    the plus outline. The pipeline is the mesh cleanup."""
    g = GltfBuilder()
    red = g.material("healthpack", emissive=[1.0, 0.10, 0.08], emissive_strength=2.2)
    s, r, d = 0.11, 0.33, 0.09  # arm half-width, reach, half-depth
    rects = [
        (-s, -s, s, s),   # hub
        (-r, -s, -s, s),  # left arm
        (s, -s, r, s),    # right arm
        (-s, s, s, r),    # top arm
        (-s, -r, s, -s),  # bottom arm
    ]
    pts, tris = [], []
    for (x0, y0, x1, y1) in rects:
        tris += quad(pts, (x0, y0, d), (x1, y0, d), (x1, y1, d), (x0, y1, d))
        tris += quad(pts, (x1, y0, -d), (x0, y0, -d), (x0, y1, -d), (x1, y1, -d))
    outline = [
        (-s, -r), (s, -r), (s, -s), (r, -s), (r, s), (s, s),
        (s, r), (-s, r), (-s, s), (-r, s), (-r, -s), (-s, -s),
    ]
    for i in range(len(outline)):
        (x0, y0), (x1, y1) = outline[i], outline[(i + 1) % len(outline)]
        tris += quad(pts, (x0, y0, -d), (x1, y1, -d), (x1, y1, d), (x0, y0, d))
    g.mesh("healthpack", [g.primitive(pts, tris, red)])
    g.write(OUT_DIR / "healthpack.gltf")


def make_powerup():
    """Boss bounty: two solid cyan chevron prisms pointing +X -- a 3D
    ">>" dash glyph. Same recipe as the medkit: front and back faces
    plus outline walls; the converter's weld and coplanar-edge drop
    leave clean chevron outlines, and the faces occlude so the glyph
    reads solid from every angle as it spins."""
    g = GltfBuilder()
    cyan = g.material("powerup", emissive=[0.35, 0.95, 1.0], emissive_strength=1.6)
    d = 0.10  # half-depth of the extrusion
    # Chevron hexagon (CCW from +Z), band 0.28 thick, pointing +X:
    #   B0 -> B1 -> TIP -> T1 -> T0 -> NOTCH
    hexagon = [
        (-0.83, -0.7),  # B0 bottom trailing
        (-0.55, -0.7),  # B1 bottom leading
        (0.25, 0.0),    # TIP leading point
        (-0.55, 0.7),   # T1 top leading
        (-0.83, 0.7),   # T0 top trailing
        (-0.03, 0.0),   # NOTCH trailing point
    ]
    pts, tris = [], []
    for dx in (0.0, 0.72):  # two chevrons: ">>"
        p = [(x + dx, y) for (x, y) in hexagon]
        (b0, b1, tip, t1, t0, notch) = p
        # Front (+Z) as two CCW quads split at the concave notch.
        tris += quad(pts, (*notch, d), (*tip, d), (*t1, d), (*t0, d))
        tris += quad(pts, (*b0, d), (*b1, d), (*tip, d), (*notch, d))
        # Back (-Z), winding reversed.
        tris += quad(pts, (*t0, -d), (*t1, -d), (*tip, -d), (*notch, -d))
        tris += quad(pts, (*notch, -d), (*tip, -d), (*b1, -d), (*b0, -d))
        # Outline walls.
        for i in range(len(p)):
            (x0, y0), (x1, y1) = p[i], p[(i + 1) % len(p)]
            tris += quad(pts, (x0, y0, -d), (x1, y1, -d), (x1, y1, d), (x0, y0, d))
    g.mesh("powerup", [g.primitive(pts, tris, cyan)])
    g.write(OUT_DIR / "powerup.gltf")


def make_boss():
    """Mini-boss: a true icosahedron cut at the equator into two models
    (boss_top / boss_bottom), each with origin at the cut plane so the
    game can raise and spin the crown independently. The equator cut
    lands exactly on band-edge midpoints (a regular decagon); each half
    is sealed with a flat decagon cap in a hotter flickering "core"
    material -- the glow you see when the boss opens to fire."""
    C = 1.15                      # circumradius
    ry, rr = C / 5 ** 0.5, 2 * C / 5 ** 0.5
    top = (0.0, C, 0.0)
    bot = (0.0, -C, 0.0)
    upper = [(rr * math.cos(math.radians(72 * k)), ry,
              rr * math.sin(math.radians(72 * k))) for k in range(5)]
    lower = [(rr * math.cos(math.radians(72 * k + 36)), -ry,
              rr * math.sin(math.radians(72 * k + 36))) for k in range(5)]
    mid = lambda a, b: tuple((x + y) / 2 for x, y in zip(a, b))
    # Decagon: D[2k] on U_k->L_k, D[2k+1] on U_{k+1}->L_k (every 36 deg).
    deca = []
    for k in range(5):
        deca.append(mid(upper[k], lower[k]))
        deca.append(mid(upper[(k + 1) % 5], lower[k]))

    def build(name, cap_up):
        g = GltfBuilder()
        shell = g.material("boss-shell", emissive=[1.0, 0.22, 0.12],
                           emissive_strength=1.5)
        core = g.material("boss-core-flicker", emissive=[1.0, 0.75, 0.30],
                          emissive_strength=2.6)
        pts, tris = [], []
        cpts, ctris = [], []

        def tri(a, b, c, into_pts=None, into_tris=None):
            p = pts if into_pts is None else into_pts
            t = tris if into_tris is None else into_tris
            base = len(p)
            p.extend([a, b, c])
            t.extend([base, base + 1, base + 2])

        if cap_up:  # top half: apex + upper ring + decagon
            for k in range(5):
                j = (k + 1) % 5
                tri(top, upper[k], upper[j])                       # crown fan
                tri(upper[k], upper[j], deca[2 * k])               # quad half
                tri(upper[j], deca[2 * k + 1], deca[2 * k])        # quad half
                tri(upper[j], deca[2 * k + 1], deca[(2 * k + 2) % 10])
            for j in range(10):                                    # cap faces -Y
                tri((0.0, 0.0, 0.0), deca[(j + 1) % 10], deca[j], cpts, ctris)
        else:       # bottom half: mirror
            for k in range(5):
                j = (k + 1) % 5
                tri(bot, lower[j], lower[k])
                tri(lower[k], lower[j], deca[(2 * k + 2) % 10])
                tri(lower[k], deca[(2 * k + 2) % 10], deca[2 * k + 1])
                tri(lower[k], deca[2 * k + 1], deca[2 * k])
            for j in range(10):                                    # cap faces +Y
                tri((0.0, 0.0, 0.0), deca[j], deca[(j + 1) % 10], cpts, ctris)

        g.mesh(name, [g.primitive(pts, tris, shell),
                      g.primitive(cpts, ctris, core)])
        g.write(OUT_DIR / f"{name}.gltf")

    build("boss_top", True)
    build("boss_bottom", False)

if __name__ == "__main__":
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    make_arena()
    make_shard()
    make_sentinel()
    make_healthpack()
    make_powerup()
    make_boss()
