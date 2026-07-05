#!/usr/bin/env python3
"""Generate the M3 corridor level (assets/corridor/): two rooms joined by a
crenellated doorway, decor line art drawn directly on the surfaces, and a
stylized plant. Aims squarely at the reference screenshot's vibe.

Layout: floor y=0, ceiling y=3; room A z∈[0,9] (player side), room B
z∈[-9,0]; divider wall at z=0 with a doorway x∈[-1.3,1.3], y∈[0,2.3].

The plant's materials are near-white on purpose: scene instances recolor it
via the multiplicative `tint`, so one model serves green and magenta plants.

Run from the repo root:  python3 tools/gen_corridor.py
"""

from pathlib import Path

from gltf_builder import GltfBuilder, LINES, LINE_LOOP, LINE_STRIP, TRIANGLES

OUT_DIR = Path(__file__).resolve().parent.parent / "assets" / "corridor"

HALF_W = 3.5   # room half-width (x)
DEPTH = 9.0    # each room's depth (z)
HEIGHT = 3.0
DOOR_HW = 1.3  # doorway half-width
DOOR_H = 2.3


def quad(points, a, b, c, d):
    base = len(points)
    points.extend([a, b, c, d])
    return [base, base + 1, base + 2, base, base + 2, base + 3]


def rect_y(y, x0, z0, x1, z1):
    """Rectangle loop on a horizontal plane."""
    return [(x0, y, z0), (x1, y, z0), (x1, y, z1), (x0, y, z1)]


def rect_x(x, z0, y0, z1, y1):
    """Rectangle loop on an x = const wall."""
    return [(x, y0, z0), (x, y1, z0), (x, y1, z1), (x, y0, z1)]


def rect_z(z, x0, y0, x1, y1):
    """Rectangle loop on a z = const wall."""
    return [(x0, y0, z), (x1, y0, z), (x1, y1, z), (x0, y1, z)]


def spiral_on_floor(cx, cz, extent, step=0.22, y=0.0):
    points = [(cx - extent, y, cz - extent)]
    x, z = cx - extent, cz - extent
    length = 2 * extent
    directions = [(1, 0), (0, 1), (-1, 0), (0, -1)]
    leg = 0
    while length > 0.12:
        dx, dz = directions[leg % 4]
        x += dx * length
        z += dz * length
        points.append((x, y, z))
        if leg % 2 == 1:
            length -= step
        leg += 1
    return points


def crenellation(x0, x1, y_base, tooth=0.22, height=0.26, z=0.0):
    """Square-tooth battlement polyline along the door lintel."""
    points = [(x0, y_base, z)]
    x, up = x0, True
    while x < x1 - 1e-6:
        nx = min(x + tooth, x1)
        y = y_base + height if up else y_base
        points.append((x, y, z))
        points.append((nx, y, z))
        x, up = nx, not up
    points.append((x1, y_base, z))
    return points


def make_corridor():
    g = GltfBuilder()
    structure = g.material("structure", base_color=[0.30, 0.85, 0.12])
    panel = g.material("panel", emissive=[0.55, 1.0, 0.10])
    door = g.material("door", emissive=[0.05, 0.75, 1.0], emissive_strength=3.0)
    floor_art = g.material("floor-art", emissive=[0.05, 1.0, 0.15], emissive_strength=1.8)

    w, h, d, dhw, dh = HALF_W, HEIGHT, DEPTH, DOOR_HW, DOOR_H

    # --- structure (occluding surfaces) ---
    pts, tris = [], []
    tris += quad(pts, (-w, 0, -d), (w, 0, -d), (w, 0, d), (-w, 0, d))      # floor
    tris += quad(pts, (-w, h, -d), (w, h, -d), (w, h, d), (-w, h, d))      # ceiling
    tris += quad(pts, (-w, 0, -d), (-w, h, -d), (-w, h, d), (-w, 0, d))    # left wall
    tris += quad(pts, (w, 0, -d), (w, h, -d), (w, h, d), (w, 0, d))        # right wall
    tris += quad(pts, (-w, 0, d), (w, 0, d), (w, h, d), (-w, h, d))        # back (player)
    tris += quad(pts, (-w, 0, -d), (w, 0, -d), (w, h, -d), (-w, h, -d))    # far wall
    # divider wall at z=0 with doorway
    tris += quad(pts, (-w, 0, 0), (-dhw, 0, 0), (-dhw, h, 0), (-w, h, 0))  # left of door
    tris += quad(pts, (dhw, 0, 0), (w, 0, 0), (w, h, 0), (dhw, h, 0))      # right of door
    tris += quad(pts, (-dhw, dh, 0), (dhw, dh, 0), (dhw, h, 0), (-dhw, h, 0))  # lintel

    primitives = [g.primitive(pts, tris, structure)]

    def loops(loop_points, material):
        primitives.append(
            g.primitive(loop_points, list(range(len(loop_points))), material, mode=LINE_LOOP)
        )

    def strip(strip_points, material):
        primitives.append(
            g.primitive(strip_points, list(range(len(strip_points))), material, mode=LINE_STRIP)
        )

    # --- door decor (cyan, drawn exactly on the z=0 plane) ---
    strip([(-dhw, 0, 0), (-dhw, dh, 0), (dhw, dh, 0), (dhw, 0, 0)], door)
    inset = 0.14
    strip(
        [(-dhw - inset, 0, 0), (-dhw - inset, dh + inset, 0),
         (dhw + inset, dh + inset, 0), (dhw + inset, 0, 0)],
        door,
    )
    strip(crenellation(-dhw - inset, dhw + inset, dh + inset), door)

    # Dashed vertical accents flanking the door (material name carries the
    # style: the converter marks any "*dash*" material's edges dashed).
    door_dash = g.material("door-accent-dash", emissive=[0.05, 0.75, 1.0], emissive_strength=2.2)
    accent_points, accent_indices = [], []
    for x in (-dhw - 0.45, dhw + 0.45, -dhw - 0.7, dhw + 0.7):
        accent_indices += [len(accent_points), len(accent_points) + 1]
        accent_points += [(x, 0.15, 0.0), (x, dh + 0.4, 0.0)]
    primitives.append(g.primitive(accent_points, accent_indices, door_dash, mode=LINES))

    # Flickering "failing light" strip on room B's ceiling.
    flicker = g.material("roomb-light-flicker", emissive=[0.55, 1.0, 0.10], emissive_strength=2.5)
    primitives.append(
        g.primitive(
            rect_y(h - 0.001, -1.4, -6.2, 1.4, -3.2),
            [0, 1, 2, 3],
            flicker,
            mode=LINE_LOOP,
        )
    )

    # --- wall panels (lime, two nested rectangles per bay) ---
    for x_wall in (-w, w):
        for z0 in (-8.4, -4.4, 0.6, 4.6):
            z1 = z0 + 3.6
            loops(rect_x(x_wall, z0 + 0.25, 0.45, z1 - 0.25, 2.55), panel)
            loops(rect_x(x_wall, z0 + 0.60, 0.80, z1 - 0.60, 2.20), panel)
    for z_wall, sign in ((d, 1), (-d, 1)):
        loops(rect_z(z_wall, -w + 0.5, 0.45, w - 0.5, 2.55), panel)
        loops(rect_z(z_wall, -w + 0.9, 0.80, w - 0.9, 2.20), panel)

    # --- floor & ceiling art ---
    for z_mid in (d / 2, -d / 2):
        z0, z1 = (0.35, d - 0.35) if z_mid > 0 else (-d + 0.35, -0.35)
        loops(rect_y(0.0, -w + 0.35, z0, w - 0.35, z1), floor_art)
        loops(rect_y(0.0, -w + 0.65, z0 + 0.3, w - 0.65, z1 - 0.3), floor_art)
        strip(spiral_on_floor(0.0, z_mid, 1.5), floor_art)
        loops(rect_y(HEIGHT, -w + 0.7, z0 + 0.35, w - 0.7, z1 - 0.35), panel)

    g.mesh("corridor", primitives)
    g.write(OUT_DIR / "corridor.gltf")


def make_plant(leaves=7):
    """Stylized plant: black-filled leaf sheets with boundary outlines around
    a drawn stem. Near-white materials — the scene's `tint` recolors it."""
    import math

    g = GltfBuilder()
    leaf_mat = g.material("leaf", base_color=[0.95, 1.0, 0.95])
    stem_mat = g.material("stem", emissive=[0.9, 1.0, 0.9])

    pts, tris = [], []
    golden = math.radians(137.5)
    for i in range(leaves):
        h = 0.20 + 0.14 * i
        theta = i * golden
        length = 1.45 - 0.12 * i
        dx, dz = math.cos(theta), math.sin(theta)
        px, pz = -dz, dx
        base = (0.0, h, 0.0)
        tip = (dx * length * 0.85, h + length * 0.75, dz * length * 0.85)
        mid = (dx * length * 0.45, h + length * 0.32, dz * length * 0.45)
        width = 0.20 * length
        s1 = (mid[0] + px * width, mid[1], mid[2] + pz * width)
        s2 = (mid[0] - px * width, mid[1], mid[2] - pz * width)
        b = len(pts)
        pts.extend([base, s1, tip, s2])
        tris += [b, b + 1, b + 2, b, b + 2, b + 3]

    stem = [(0.0, 0.0, 0.0), (0.01, 0.5, -0.01), (0.04, 0.9, 0.02), (0.02, 1.18, 0.05)]
    g.mesh(
        "plant",
        [
            g.primitive(pts, tris, leaf_mat),
            g.primitive(stem, list(range(len(stem))), stem_mat, mode=LINE_STRIP),
        ],
    )
    g.write(OUT_DIR / "plant.gltf")


if __name__ == "__main__":
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    make_corridor()
    make_plant()
