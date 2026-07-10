# File formats

Three formats cross the engine boundary: the `.vec` binary model, the
scene `.ron`, and the animation `.anim.ron`. All three are stable,
versioned-or-tiny, and human-diffable where text.

## `.vec` — vector model (binary)

Written/read by `vex_core::VecModel::{save, load}`. Little-endian, chunked:

```
"VEC1"  u32 version (= 1)
then chunks, each:  [u8;4] tag, u32 byte length, payload

PALT  u32 count, count × 3×f32     linear RGB palette (HDR: values may exceed 1.0)
VERT  u32 count, count × 3×f32     positions
EDGE  u32 count, count × {
        u32 a, u32 b               vertex indices
        u8 palette                 index into PALT
        u8 kind                    0 = Always, 1 = Smooth
        u8 style                   bitflags: 1 = dashed, 2 = flicker
        u8 pad
        f32 intensity              per-edge brightness multiplier
        3×f32 n1, 3×f32 n2         adjacent face normals (Smooth edges)
      }
OCCL  u32 count, count × u32       triangle indices into VERT (depth-only mesh)
AABB  6×f32                        min, max
```

Readers reject bad magic and absurd counts (corrupt-file guard). Unknown
chunk tags are skipped, so new chunk types can be added without breaking
old readers — bump `VEC_VERSION` only for changes existing readers would
misparse.

### Edge semantics

- **`Always`** — boundaries, creases, material borders, authored decor
  lines. Drawn every frame via `edge_segments(kind, intensity_scale)`.
- **`Smooth`** — an edge on a curved surface, drawn *only while it is a
  silhouette*. It carries both adjacent face normals; at runtime
  `silhouette_segments(..)` keeps it when one face points toward the eye
  and the other away. This is what makes a low-poly sphere read as a
  circle-ish outline instead of a triangle mess.

### Palette rules

Colors are **linear RGB**, premultiplied with emissive strength at
convert time — so "hot" is stored in the palette, and the runtime glow
dial rescales it (see [Rendering](rendering.md)). One model may hold at
most **255 distinct colors** (`palette` is a byte).

## Producing `.vec`

`vex-convert` is both a CLI and a library:

```sh
vex-convert model.gltf [-o model.vec] [--crease 30]
```

Pipeline: load glTF → weld duplicate vertices → classify every mesh edge
(boundary / crease over `--crease` degrees / material border → `Always`;
the rest → `Smooth` with stored normals) → intern colors into the
palette → emit occluder triangles + AABB.

Authoring conventions the converter understands:

- **Color**: a material's *emissive* color × strength
  (`KHR_materials_emissive_strength`, i.e. Blender's emissive slider)
  wins; base color is the fallback. Strength > 1 becomes HDR glow.
- **Style by material name**: a material whose name contains `dash` or
  `flicker` (e.g. `trim-dash`, `sign_flicker`) marks its edges dashed /
  flickering. Cheap to author anywhere, survives export.

The library entry point `build_model(&SourceGeometry, &ConvertOptions)`
accepts geometry from *any* source — the level editor feeds it triangles
directly and gets welded outlines, silhouettes and collision for free.

## Scene `.ron`

Loaded by `vex_engine::load_scene` into a `BakedScene` (all instances
pre-transformed into one segment/occluder pool with per-instance ranges
and AABBs for culling). Paths are relative to the scene file.

```ron
(
    models: { "corridor": "corridor.vec", "plant": "plant.vec" },
    instances: [
        (model: "corridor"),
        (model: "plant", position: (2.7, 0.0, 7.6), yaw_deg: 15.0,
         scale: 1.35, intensity: 1.0, tint: (0.1, 1.0, 0.25)),
    ],
    player: (position: (0.0, 0.0, 7.6), yaw_deg: 0.0),
    weapon: Some((model: "sword")),      // optional viewmodel
    fog_density: 0.09,                    // 0.0 = no fog
    post: (glow: 0.45, bloom_strength: 0.12, exposure: 1.0, crt: 0.0),
)
```

Instance fields beyond `model` are optional with sane defaults. `tint`
multiplies the palette (author near-white assets, recolor per instance);
`intensity` scales brightness. The `post:` block seeds `PostSettings`.

## Animation `.anim.ron`

Loaded by `vex_engine::Clip::load` (or `from_str`), played by
`AnimPlayer`. Rigid-transform keyframes — no skeletons:

```ron
Clip(
    duration: 6.0,
    loop_mode: Loop,            // Once | Loop | PingPong
    tracks: [
        Track(channel: RotY,      easing: Linear, keys: [(0.0, 0.0), (6.0, 360.0)]),
        Track(channel: PosY,      easing: Smooth, keys: [(0.0, 0.0), (3.0, 0.18), (6.0, 0.0)]),
        Track(channel: Intensity, easing: Smooth, keys: [(0.0, 1.0), (3.0, 1.8), (6.0, 1.0)]),
    ],
)
```

- **Channels**: `PosX/PosY/PosZ`, `RotX/RotY/RotZ` (degrees, composed in
  YXZ order), `Scale` (uniform), `Intensity` (brightness multiplier —
  not part of the matrix; apply it to segment colors).
- **Easing** per track: `Step`, `Linear`, `Smooth` (smoothstep).
- Keys are `(time_seconds, value)` and must be in ascending time order.
- `Clip::sample(t)` → `Pose { translation, rotation_deg, scale,
  intensity }`; `Pose::transform()` builds the `T·R·S` matrix to apply to
  a model's segments *and* its occluder vertices (pose both, or shadows
  detach from the object).

The viewer previews clips live: `cargo run -p viewer -- model.vec
--anim clip.anim.ron`, hot-reloading the clip on save.
