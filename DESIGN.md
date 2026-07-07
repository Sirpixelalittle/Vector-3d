# vector3d — Design Document

A 3D game engine whose entire visual language is bright vector strokes on black:
the look of vector-CRT arcade machines (Battlezone, Tempest, Star Wars '83) and
early wireframe 3D (Elite, Mercenary, Driller), but with modern conveniences —
real occlusion, glow, arbitrary meshes authored in Blender, high framerates.

Lines are the primitive. Surfaces exist only to hide other lines.

---

## 1. Reading the reference image

The target screenshot (neon corridor, `~/Pictures/dea9Nc.png`) tells us more
than "wireframe." Decomposed:

| Observation | Implication for the engine |
|---|---|
| Walls/leaves hide what's behind them; leaves are solid black shapes with green outlines | **Hidden-line rendering**: opaque black-filled geometry writes depth, edges draw on top. Not see-through wireframe. |
| Line width is constant in screen space (~1–2 px), near or far | Lines are expanded to screen-space quads; width in pixels, not world units. |
| The room beyond the door is dimmer than the door frame | **Depth cueing** — per-vertex intensity falloff toward black with distance (classic vector-monitor trick, also fights far-line moiré). |
| Greek-key wall patterns and the floor spiral are flat decoration, not geometry creases | The converter must ingest **authored line art** (glTF `LINES` primitives / Blender loose edges), not just extract edges from triangles. |
| Dashed vertical lines by the door frame | Per-edge **dash style**, parameterized in world units. |
| Door frame is noticeably brighter cyan with a halo | HDR per-edge **intensity** + bloom. Intensity > 1.0 = glows. |
| Small fixed palette: greens, cyan/blue, magenta, red | **Palette-driven color**, not per-pixel material shading. |
| Green weapon in the corner doesn't clip into walls | Separate **weapon render layer** (depth cleared, no depth-cue fog). |
| Red crosshair + "Health 100" text | HUD drawn with the same stroke renderer (vector/Hershey font). |

### Aesthetic pillars

1. **Lines are the only visible thing.** Triangles are invisible occluders.
2. **Hidden-line removal is always on.** Readability over wireframe purism.
3. **Screen-constant stroke width**, anti-aliased.
4. **Small HDR palette; intensity is a first-class property.** Bloom sells the phosphor.
5. **Depth cueing to black.** Distance = dimness.
6. **Everything speaks stroke** — world, weapon, HUD, text.

Feature test: if it doesn't serve lines-on-black, it doesn't go in the engine.

---

## 2. The core technique (why this looks right)

Naive wireframe (draw all edges, no occlusion) reads as visual soup — 1980s
games lived with it because they had no choice. The screenshot look is
**hidden-line rendering**, a two-pass trick that's standard NPR practice:

```
Pass 1  OCCLUDERS   triangle mesh → depth buffer only (color stays black).
                    Small positive depth bias pushes surfaces slightly *away*.

Pass 2  EDGES       line segments → screen-space quads, depth-tested (LESS-EQUAL)
                    against pass 1. Lines on a surface survive; lines behind
                    a surface are eaten by the depth test.
```

The depth bias on the *occluders* (not the lines) is what prevents an edge
from z-fighting against the very face it borders. This needs per-scene tuning
(constant + slope-scaled bias) and grazing-angle testing — it is the one
finicky part of the whole approach, and it's well-trodden territory.

### Full frame pipeline

```
 ┌─────────────┐   ┌─────────────┐   ┌──────────────┐   ┌────────────┐
 │ 1 occluders  │ → │ 2 edges     │ → │ 3 weapon     │ → │ 4 HUD      │
 │ depth only,  │   │ quads, AA,  │   │ clear depth, │   │ ortho      │
 │ +depth bias  │   │ palette ×   │   │ redraw occl+ │   │ strokes,   │
 │              │   │ intensity,  │   │ edges, no    │   │ Hershey    │
 │              │   │ depth cue,  │   │ depth cue    │   │ text       │
 │              │   │ dashes      │   │              │   │            │
 └─────────────┘   └─────────────┘   └──────────────┘   └────────────┘
                          all into one HDR (RGBA16F) target
                                        ↓
                    ┌───────────────────────────────────┐
                    │ 5 POST: mip-chain bloom (dual      │
                    │ filter), exposure/tonemap,         │
                    │ optional CRT (barrel, chroma)      │
                    └───────────────────────────────────┘
```

HUD draws into the HDR buffer *before* post so it glows like everything else
(the red health text in the reference has a faint halo).

### Line rendering details

- **No native wide lines.** Vulkan/wgpu line primitives are 1 px and
  unreliable across drivers. Each segment becomes an instanced quad
  (2 triangles), expanded in clip space along the screen-space perpendicular.
- **Round caps via distance field** in the fragment shader — polyline joins
  look clean without miter math.
- **Anti-aliasing**: 1 px smoothstep falloff at the quad's edge. No MSAA needed.
- **Dashes**: cumulative arc length along each polyline chain is baked into
  the vertex stream at load; the pattern is world-unit-stable (screen-space
  dashes crawl when the camera moves).
- **Depth cueing**: `intensity *= exp(-k · view_distance)`, computed per
  fragment from interpolated world position (per-vertex would fade long
  segments incorrectly — a grid line passing near the camera would take its
  endpoints' dimness). Doubles as anti-moiré for dense far geometry.
- **Runtime silhouettes** (curved objects): edges tagged "smooth" carry both
  adjacent face normals; an edge is a silhouette iff
  `dot(n1, v) · dot(n2, v) ≤ 0` (v = midpoint→camera). CPU per frame is fine
  at retro edge counts; move to a compute pass only if profiling demands.

---

## 3. The `.vec` model format

Binary, little-endian, chunked. One `.vec` per mesh.

```
"VEC1" magic + version
PALT   palette: N × RGB half-float (values > 1.0 glow)
VERT   M × f32×3 positions (welded, shared by edges)
EDGE   E × { a: u32, b: u32,            vertex indices
             palette: u8,
             style: u8,                  solid | dash | silhouette-candidate
             intensity: f16,
             n1, n2: oct16 }             adjacent face normals (silhouette test)
CHAIN  polyline runs (edge index ranges) — dash continuity, fewer uploads
OCCL   occluder triangle indices into VERT (depth-only rendering needs no
       normals/uvs, so sharing the welded table is safe and halves the file)
AABB   bounds for frustum culling
```

~40 bytes/edge in v1 (f32 normals; oct16/f16 compression is a later
optimization if files ever get heavy). Chunks are tagged and unknown tags
are skipped, so CHAIN (polylines for dashes) can land without a version
bump.

Design intent: the file carries **both** the visible lines *and* the invisible
occluder mesh — they are different geometry with different requirements, and
the pairing is what makes the format "a vector model" rather than a mesh.

---

## 4. `vex-convert` — the model-to-vectors converter

**This is the content pipeline, not a utility.** Instead of hand-typing vertex
tables like 1982, any mesh from Blender or an asset store becomes vector art:

```
Blender ──glTF──▶ vex-convert ──.vec──▶ engine
   (models, levels, loose-edge line art, materials→palette)
```

Input: glTF 2.0 primary (Blender-native, has everything we need). OBJ later
if ever needed.

### Edge classification algorithm

```
1. Weld vertices          ε = 1e-5 × bbox diagonal
2. Build edge→face adjacency map
3. For each edge:
     1 adjacent face                        → BOUNDARY   (always drawn)
     adjacent faces differ in material      → MATERIAL   (always drawn)
     dihedral angle > θ  (default 30°)      → CREASE     (always drawn)
     else                                   → SMOOTH     (drawn only when
                                                          silhouette at runtime;
                                                          store both normals)
4. glTF LINES primitives (Blender "loose edges" export)
                                            → DECOR      (always drawn)
5. Chain edges into polylines (shared endpoints, same style, low turn angle)
6. Map material base colors → nearest palette entry (or --keep-colors)
7. Emit edges + chains + original triangles as occluder + AABB
```

Step 4 is load-bearing for the aesthetic: the floor spiral and Greek-key
patterns in the reference are *drawn*, not modeled. An artist sketches loose
edges directly on surfaces in Blender and they pass straight through.

```
vex-convert corridor.gltf -o assets/corridor.vec --crease 30 --palette neon.pal
```

### Acceptance tests

- Cube → exactly 12 edges, all crease.
- Smooth cylinder → 2 rim circles always-drawn; barrel edges all
  silhouette-candidates (nothing drawn on the barrel until viewed side-on).
- Single-sided plane (leaf) → 4 boundary edges.
- Suzanne + a plant with loose-edge veins → eyeball in the viewer.

### Known gotcha

Faceted exports (all-hard normals) make *every* edge a crease. Mitigation:
classification uses face-geometry dihedral angles from welded positions, never
authored vertex normals; plus a `--max-edges` sanity warning.

---

## 5. Runtime architecture

Rust workspace:

```
vector3d/
  Cargo.toml            workspace
  crates/
    vex-core/           .vec read/write, palette, Hershey stroke font, shared types
    vex-convert/        CLI: glTF → .vec        (depends on core + gltf crate)
    vex-render/         wgpu renderer: 5-pass pipeline, buffer management
    vex-engine/         winit shell: loop, input, time, cameras, scene, culling
  examples/
    01-cube/            hardcoded cube, fly camera         (M0/M1)
    02-viewer/          load .vec, orbit camera, hot-reload (M2)
    03-corridor/        FPS walkthrough à la the reference  (M3)
  assets/               palettes, test models
```

- **Math**: `glam`. **Windowing**: `winit`. **GPU**: `wgpu` (Vulkan on this
  machine; WebGPU browser demo becomes possible later for free).
- **Scene**: a plain `Vec<Instance>` — `{ model handle, transform, palette
  override, intensity mul, layer }` + camera. **No ECS yet** (YAGNI; adopt
  `hecs`/`bevy_ecs` only when gameplay code creates real pressure).
- **Renderer API**: retained-light. Static edge/occluder buffers uploaded once
  per model; per-frame streaming buffer for instance transforms + dynamic
  silhouette edges. `renderer.draw(&instances, &camera, &hud_strokes)`.
- **Culling**: frustum vs instance AABB. Plus screen-length LOD: skip edges
  whose projected length < ~1.5 px (moiré control, essentially free).
- **Levels**: authored in Blender as a glTF scene; converter emits one `.vec`
  per mesh + a RON scene file of instances. Blender is the level editor.
- **Collision (M3)**: capsule vs occluder triangles through a uniform grid,
  slide response. Enough for corridors; no physics engine.
- **Text**: Hershey simplex stroke font baked into `vex-core` — text is
  polylines like everything else.

---

## 6. Milestones

**M0 — Window & lines** ✅ *2026-07-03*
winit + wgpu init, hardcoded cube as raw line list, fly camera.
Exit: colored cube edges on black at 144 fps.
*(Landed with extras: the real quad-expansion AA line shader, near-plane
clipping, additive blending, headless `--screenshot` verification path.)*

**M1 — The look** ✅ *2026-07-04*
Occluder pass + depth bias, quad-expanded AA lines, palette + HDR intensity,
depth cueing. Z-fight tuning at grazing angles.
Exit: overlapping spinning shapes with correct hidden-line occlusion.
*(Verified: orbiting icosahedron occludes/is occluded by spinning cube;
grazing-angle shot along a face shows continuous edges, no stitching, no
poke-through at bias constant=2 / slope_scale=2.0.)*

**M2 — The converter** ✅ *2026-07-04* ← the "model → vectors" milestone
`vex-convert` per §4, `.vec` load/save, viewer example with file hot-reload.
Exit: Blender Suzanne and a loose-edge-decorated model render correctly.
*(Verified: all §4 acceptance numbers exact — plane 4 boundary, cylinder
48 crease + 24 smooth, decorated cube 12 crease + spiral decor drawn
on-surface without z-fighting; Suzanne welds 11 808→2 012 verts, edge
accounting matches Euler's V+F−2 exactly; hot reload confirmed live.
Viewer accepts .gltf directly (in-process convert), so Blender→save→
refresh needs no manual convert step.)*

**M3 — Walk the corridor** ✅ *2026-07-04*
Scene format, frustum culling, FPS camera + capsule collision, weapon layer.
Exit: walkthrough demo comparable to the reference screenshot.
*(Landed: RON scenes with per-instance tint/intensity — one white plant
model serves green and magenta variants; scenes bake to world space once,
recording per-instance buffer ranges so culling = drawing visible slices;
collision reuses the occluder mesh via a hash-grid capsule slide (tests
caught and fixed a push-through-the-wall tie-break bug); weapon layer =
depth-clear + second camera binding, with the user's converted sword.glb
auto-fit into the hand. Spawn-view screenshot holds up next to the
reference image.)*

**M4 — Motion & style** ✅ *2026-07-04*
Runtime silhouettes, world-space dashes, per-edge intensity animation
(flicker/pulse), stroke-font HUD (health counter, crosshair).
*(Landed: silhouette test per §2 runs in model space — the eye is
transformed in, so sign products survive rotation + uniform scale; the
cylinder grows barrel edges exactly at its horizon and Suzanne finally has
her outline. Dashes are perspective-correct world-unit patterns; flicker
phases per instance so lights don't blink in sync. Styles are authored by
material-name convention ("*dash*", "*flicker*"). The HUD font is an
original angular stroke font (not Hershey) — more Atari-vector than
plotter, zero-length segments render as dots via the round caps.)*

**M5 — Phosphor** ✅ *2026-07-04*
Mip-chain bloom, exposure, optional CRT effects behind a flag (barrel
distortion, chromatic offset, endpoint "beam dwell" dots).
Exit: side-by-side with the reference image and it holds up.
*(Landed: RGBA16F scene target; threshold-less half-res bloom chain (box
down, additive tent up, ≤6 levels); `1 − exp(−x·exposure)` soft-clip
tonemap; CRT = barrel + chroma + vignette on a 0..1 dial ([C] in the
corridor, `--crt` headless). Beam-dwell dots were already emergent from
additive caps. Glow authoring: converter honors
KHR_materials_emissive_strength, so Blender's emissive-strength slider
sets *relative* strengths; corridor door authors 3×. HUD renders pre-post
and glows, per the original §2 diagram. Exit met — the money shot holds up
next to the reference.
Addendum: brightness is engine-owned, not asset-owned. A hue-preserving
`glow` dial (camera uniform, applied in the line shader) compresses how
far palette colors may exceed 1.0 — assets author ratios, the scene's
`post:` block + live keys ([ ] glow · - = bloom · 9 0 exposure · C CRT)
decide what reaches the eye. Defaults tuned gentle: glow 0.5, bloom 0.14,
exposure 1.0.)*

**M6 — Playable demo: the arena** ✅ *2026-07-04*
`cargo run -p arena` — wave-based sword-fighting in an octagonal neon pit
(Battlezone mountain horizon, spawn gates, pillars). Two enemy types built
through the normal asset pipeline (magenta shard swarms, amber sentinels
whose floating eye watches you); grip-pivoted left→right slash with cone
hit detection; line-burst death particles; stroke-text banners, health/
wave/score HUD, damage flash + camera shake; game over + restart. Game
rules live in a pure, unit-tested module (`examples/04-arena/src/game.rs`);
deterministic RNG makes headless `--demo` screenshots reproducible.

**M7 — Sound** ✅ *2026-07-05*
`vex-audio`: 3D spatial audio on kira 0.12 (listener follows the camera;
positional one-shots via transient spatial tracks with linear distance
attenuation, 2–42 m). Every SFX is procedurally synthesized at startup —
square/saw/sine sweeps and xorshift noise bursts, deterministic, zero
audio files (era-correct, and free bytes on the web build). Games emit
`GameEvent`s; the app drains them into the mixer. Audio starts on the
first captured click, which doubles as the browser autoplay gesture.
Works native and wasm.

**M8 — Level editor** ✅ *2026-07-06*
`examples/05-editor`: in-engine block-out tool. Fly camera, grid-snapped
ghost preview, four primitives (box / cylinder / wedge / doorframe) with
stepped hue / saturation / glow dials, aim-to-delete with AABB highlight,
undo, RON save (`F5`) and `.vec` export (`F6`) through
`vex_convert::build_model` — the same weld/classify pipeline as Blender
content, so exports get silhouettes, welded composite outlines, and
collision-ready occluders. Preview renders through the real pass chain
(HDR bloom, glow dial, CRT), palette capped at 255 combos with graceful
refusal. Blender stays the hero-asset tool; this is for spaces.

**M9 — Animation clips** ✅ *2026-07-06*
`vex_engine::anim`: keyframed rigid-transform clips in hand-editable RON
(`.anim.ron`). Channels: PosX/Y/Z, RotX/Y/Z (degrees, YXZ), uniform
Scale, and Intensity — line brightness as a first-class animatable
channel. Step/Linear/Smooth easing per track, Once/Loop/PingPong
playback, `Clip::sample(t) → Pose`, `Pose::transform()` composes onto
any base matrix; layering is plain code (sample two clips, multiply).
`AnimPlayer` owns time/speed/finished. The viewer previews clips
(`--anim file`, `--time T` headless) with hot-reload on save, sharing
playback time across reloads so key tweaks don't restart the motion.
Games keep gameplay-driven motion in code; clips are for authored feel.

**M6+ — Stretch**
SVG frame export (posters/marketing shots), WebGPU browser demo,
oscilloscope/ILDA laser output (a *true* vector display backend — the
renderer's "list of colored segments" model makes this a real possibility),
Blender addon for palette tagging, audio (chunky synth beeps).

---

## 7. Stack rationale & alternatives considered

**Chosen: Rust + wgpu + winit + glam + `gltf`.**
Memory safety for a long-lived hobby codebase; wgpu gives Vulkan today and a
shareable WebGPU build later; a cargo workspace fits the engine + converter
tool pairing; the glTF ecosystem is mature.

| Alternative | Verdict |
|---|---|
| Bevy plugin (custom render node) | Faster gameplay scaffolding, but fights you on a fully custom renderer, and this project *is* the renderer. Revisit only if gameplay needs balloon. |
| C + OpenGL | Simplest possible start; worse long-term tooling and no web target. |
| TypeScript + WebGL | Fastest first demo, weakest as a native engine. The WebGPU export path covers the "shareable demo" itch anyway. |

The design above is ~90% stack-agnostic — only §5 changes if this call is
revisited.

---

## 8. Risks

- **Depth-bias tuning** — the one genuinely finicky bit; budget time in M1.
- **Moiré/aliasing from dense distant lines** — mitigated by depth cueing +
  screen-length LOD; verify against the floor-spiral case specifically.
- **Faceted source assets** — handled in converter (§4); worst case is ugly,
  not broken.
- **Scope creep into a general engine** — pillar test (§1) is the gate.
  Collision stays simple, no PBR temptations, no scene-graph astronautics.

## 9. Prior art worth stealing from

- **Battlezone / Star Wars (Atari vector CRTs)** — palette, glow, HUD language.
- **Elite (1984)** — proof that hidden-line-ish rendering was always the goal
  (it culled backfaces per object because true HLR was unaffordable).
- **Blender Freestyle** — the same edge taxonomy (silhouette/crease/border/
  material) in production; validates §4's classification.
- **NPR literature** ("Suggestive Contours", Real-Time Rendering ch. on line
  rendering) — silhouette math, bias techniques.
- **Hershey fonts (1967, public domain)** — stroke text that predates raster
  displays; period-perfect.
