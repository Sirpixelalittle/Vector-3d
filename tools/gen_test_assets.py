#!/usr/bin/env python3
"""Generate self-contained glTF test models for the vex-convert acceptance
tests (DESIGN.md §4), without needing Blender:

  plane.gltf           single-sided quad     → 4 boundary edges
  cylinder.gltf        24-segment cylinder   → 48 crease rims + 24 smooth barrel
  decorated-cube.gltf  cube + LINE_STRIP spiral drawn on its top face

Run from the repo root:  python3 tools/gen_test_assets.py
"""

import math
from pathlib import Path

from gltf_builder import GltfBuilder, LINE_STRIP, TRIANGLES

OUT_DIR = Path(__file__).resolve().parent.parent / "assets" / "test"

def make_plane() -> None:
    g = GltfBuilder()
    mat = g.material("panel", base_color=[0.9, 0.95, 1.0])
    points = [(-1, 0, -1), (1, 0, -1), (1, 0, 1), (-1, 0, 1)]
    g.mesh("plane", [g.primitive(points, [0, 1, 2, 0, 2, 3], mat)])
    g.write(OUT_DIR / "plane.gltf")


def make_cylinder(segments: int = 24) -> None:
    g = GltfBuilder()
    mat = g.material("barrel", base_color=[0.55, 1.0, 0.1])
    bottom = [
        (math.cos(2 * math.pi * i / segments), -1.0, math.sin(2 * math.pi * i / segments))
        for i in range(segments)
    ]
    top = [(x, 1.0, z) for (x, _, z) in bottom]
    points = bottom + top + [(0.0, -1.0, 0.0), (0.0, 1.0, 0.0)]
    bc, tc = 2 * segments, 2 * segments + 1
    tris = []
    for i in range(segments):
        j = (i + 1) % segments
        b_i, b_j, t_i, t_j = i, j, segments + i, segments + j
        tris += [b_i, b_j, t_j, b_i, t_j, t_i]  # side quad
        tris += [bc, b_j, b_i]                  # bottom cap fan
        tris += [tc, t_i, t_j]                  # top cap fan
    g.mesh("cylinder", [g.primitive(points, tris, mat)])
    g.write(OUT_DIR / "cylinder.gltf")


def spiral_points(extent: float = 0.8, step: float = 0.25, y: float = 1.0):
    """Inward rectangular spiral on the y=1 plane — drawn line art, exactly
    on the cube's top face (the depth-bias decal torture test)."""
    points = [(-extent, y, -extent)]
    x, z = -extent, -extent
    length = 2 * extent
    directions = [(1, 0), (0, 1), (-1, 0), (0, -1)]
    leg = 0
    while length > 0.1:
        dx, dz = directions[leg % 4]
        x += dx * length
        z += dz * length
        points.append((x, y, z))
        if leg % 2 == 1:
            length -= step
        leg += 1
    return points


def make_decorated_cube() -> None:
    g = GltfBuilder()
    body = g.material("body", base_color=[0.02, 1.0, 0.1])
    neon = g.material("neon", emissive=[0.05, 0.75, 1.0])
    h = 1.0
    points = [
        (-h, -h, -h), (h, -h, -h), (h, -h, h), (-h, -h, h),
        (-h, h, -h), (h, h, -h), (h, h, h), (-h, h, h),
    ]
    tris = [
        0, 1, 2, 0, 2, 3,
        4, 6, 5, 4, 7, 6,
        0, 4, 5, 0, 5, 1,
        3, 2, 6, 3, 6, 7,
        0, 3, 7, 0, 7, 4,
        1, 5, 6, 1, 6, 2,
    ]
    spiral = spiral_points()
    g.mesh(
        "decorated-cube",
        [
            g.primitive(points, tris, body),
            g.primitive(spiral, list(range(len(spiral))), neon, mode=LINE_STRIP),
        ],
    )
    g.write(OUT_DIR / "decorated-cube.gltf")


if __name__ == "__main__":
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    make_plane()
    make_cylinder()
    make_decorated_cube()
