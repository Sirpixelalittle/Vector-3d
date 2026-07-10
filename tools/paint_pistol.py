"""Paint the pistol glb's parts and regenerate assets/pistol.vec.

The source model (a sci-fi water pistol) ships as a single untextured
0.8-grey material, so the converted strokes come out pure white. It does
carry eight named part meshes, though — so instead of hand-classifying
edges in the .vec, this patches one material per part into the glb
(emissive cyan for the tank windows, tinted metals for the body, per the
maintainer's reference render) and lets vex-convert's normal rules do
the rest: emissive×strength → HDR stroke color, baseColor otherwise.

    python3 tools/paint_pistol.py [source.glb]   # default: assets/pistol.glb

Idempotent — it rewrites the materials array, so re-running on the
already-painted glb is the retuning loop: edit PARTS below, rerun,
look at a screenshot.
"""

import json
import struct
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Part name -> material. "emissive" parts glow (strength > 1 blooms);
# "base" parts are plain tinted strokes. Colors are linear RGB.
# This model is DENSE (113k edges) — on screen its strokes overlap and
# stack additively, so per-stroke colors sit far darker than a sparse
# model would need; the overlap does the brightening.
PARTS = {
    "Body":       {"base": [0.19, 0.23, 0.13]},   # olive-grey receiver
    "BodyCover":  {"base": [0.19, 0.23, 0.13]},
    "TopCover":   {"base": [0.15, 0.18, 0.24]},  # bluish gunmetal
    "Nozzle":     {"base": [0.15, 0.18, 0.24]},
    "UnderCover": {"base": [0.15, 0.18, 0.11]},
    "GripCover":  {"base": [0.075, 0.08, 0.07]},  # dark grip
    "Trigger":    {"base": [0.17, 0.175, 0.19]},  # bare steel
    "Water_Tank": {"emissive": [0.22, 0.60, 0.65], "strength": 1.0},  # the glow
}


def main() -> None:
    src = Path(sys.argv[1]) if len(sys.argv) > 1 else REPO / "assets/pistol.glb"
    data = src.read_bytes()
    assert data[:4] == b"glTF", f"{src} is not a GLB"
    json_len = struct.unpack("<I", data[12:16])[0]
    assert data[16:20] == b"JSON"
    gltf = json.loads(data[20:20 + json_len])
    rest = data[20 + json_len:]  # BIN chunk (and any trailing chunks), untouched

    materials = []
    index = {}
    for name, spec in PARTS.items():
        mat = {"name": name}
        if "emissive" in spec:
            mat["emissiveFactor"] = spec["emissive"]
            mat["extensions"] = {
                "KHR_materials_emissive_strength": {"emissiveStrength": spec["strength"]}
            }
        else:
            mat["pbrMetallicRoughness"] = {"baseColorFactor": spec["base"] + [1.0]}
        index[name] = len(materials)
        materials.append(mat)
    gltf["materials"] = materials

    used = set(gltf.get("extensionsUsed", []))
    used.add("KHR_materials_emissive_strength")
    gltf["extensionsUsed"] = sorted(used)

    unmatched = []
    for mesh in gltf["meshes"]:
        name = mesh.get("name", "")
        if name not in index:
            unmatched.append(name)
            continue
        for prim in mesh["primitives"]:
            prim["material"] = index[name]
    if unmatched:
        sys.exit(f"unpainted meshes (add them to PARTS): {unmatched}")

    body = json.dumps(gltf, separators=(",", ":")).encode()
    body += b" " * (-len(body) % 4)  # GLB chunks are 4-byte aligned
    out = bytearray()
    out += b"glTF" + struct.pack("<II", 2, 12 + 8 + len(body) + len(rest))
    out += struct.pack("<I", len(body)) + b"JSON" + body
    out += rest

    dst = REPO / "assets/pistol.glb"
    dst.write_bytes(bytes(out))
    print(f"painted {len(PARTS)} parts -> {dst}")

    # High-poly source: the default 30° crease keeps ~21k always-drawn
    # edges, which stack additively on screen and saturate the viewmodel
    # to white. A steeper crease keeps only the real panel lines.
    crease = "50"
    subprocess.run(
        ["cargo", "run", "-q", "-p", "vex-convert", "--",
         str(dst), "-o", str(REPO / "assets/pistol.vec"), "--crease", crease],
        cwd=REPO, check=True,
    )


if __name__ == "__main__":
    main()
