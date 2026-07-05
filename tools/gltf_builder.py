#!/usr/bin/env python3
"""Minimal glTF 2.0 writer shared by the asset generators. Embeds buffers
as base64 data URIs; u32 indices; positions carry mandatory min/max."""

import base64
import json
import struct
from pathlib import Path

F32 = 5126
U32 = 5125
TRIANGLES = 4
LINES = 1
LINE_STRIP = 3
LINE_LOOP = 2


class GltfBuilder:
    def __init__(self):
        self.blob = bytearray()
        self.views = []
        self.accessors = []
        self.meshes = []
        self.materials = []
        self.nodes = []
        self.extensions_used = set()

    def _view(self, data: bytes) -> int:
        while len(self.blob) % 4:
            self.blob.append(0)
        self.views.append(
            {"buffer": 0, "byteOffset": len(self.blob), "byteLength": len(data)}
        )
        self.blob.extend(data)
        return len(self.views) - 1

    def positions(self, points) -> int:
        data = b"".join(struct.pack("<3f", *p) for p in points)
        lo = [min(p[i] for p in points) for i in range(3)]
        hi = [max(p[i] for p in points) for i in range(3)]
        self.accessors.append(
            {
                "bufferView": self._view(data),
                "componentType": F32,
                "count": len(points),
                "type": "VEC3",
                "min": lo,
                "max": hi,
            }
        )
        return len(self.accessors) - 1

    def indices(self, values) -> int:
        data = b"".join(struct.pack("<I", v) for v in values)
        self.accessors.append(
            {
                "bufferView": self._view(data),
                "componentType": U32,
                "count": len(values),
                "type": "SCALAR",
            }
        )
        return len(self.accessors) - 1

    def material(self, name, base_color=None, emissive=None, emissive_strength=None) -> int:
        material = {"name": name, "pbrMetallicRoughness": {}}
        if base_color:
            material["pbrMetallicRoughness"]["baseColorFactor"] = list(base_color) + [1.0]
        if emissive:
            material["emissiveFactor"] = list(emissive)
        if emissive_strength is not None:
            # Blender's emissive-strength slider; values > 1 bloom in-engine.
            material["extensions"] = {
                "KHR_materials_emissive_strength": {"emissiveStrength": emissive_strength}
            }
            self.extensions_used.add("KHR_materials_emissive_strength")
        self.materials.append(material)
        return len(self.materials) - 1

    def mesh(self, name, primitives) -> None:
        self.meshes.append({"name": name, "primitives": primitives})
        self.nodes.append({"mesh": len(self.meshes) - 1})

    def primitive(self, points, indices, material, mode=TRIANGLES) -> dict:
        return {
            "attributes": {"POSITION": self.positions(points)},
            "indices": self.indices(indices),
            "material": material,
            "mode": mode,
        }

    def write(self, path: Path) -> None:
        uri = "data:application/octet-stream;base64," + base64.b64encode(
            bytes(self.blob)
        ).decode("ascii")
        doc = {
            "asset": {"version": "2.0", "generator": "vector3d gen_test_assets"},
            "scene": 0,
            "scenes": [{"nodes": list(range(len(self.nodes)))}],
            "nodes": self.nodes,
            "meshes": self.meshes,
            "materials": self.materials,
            "buffers": [{"uri": uri, "byteLength": len(self.blob)}],
            "bufferViews": self.views,
            "accessors": self.accessors,
        }
        if self.extensions_used:
            doc["extensionsUsed"] = sorted(self.extensions_used)
        path.write_text(json.dumps(doc, indent=1))
        print(f"wrote {path} ({path.stat().st_size} bytes)")
