# vector3d

A 3D engine that draws nothing but glowing lines — the look of vector CRTs
(Battlezone, Tempest, the '83 Star Wars cabinet) with the things those
machines could never afford: true hidden-line occlusion, bloom, arbitrary
meshes from Blender, and a playable arena shooter —
**[play it in your browser](https://chainsawqueen1.itch.io/vector3d-arena)** —
that runs native or as a 1.3 MB WebGPU wasm.

![the arena](docs/arena.png)

Everything visible in that frame is a stroke: the walls, the enemies, the
pistol, the muzzle flash, the medkit, the font. Surfaces exist only to
*occlude* the lines behind them — they render into the depth buffer, black
on black.

## Play it

**Browser** (Chrome/Edge, WebGPU): **[play on itch.io](https://chainsawqueen1.itch.io/vector3d-arena)** —
or build it yourself with `tools/build_web.sh` and
`python3 -m http.server 8080 -d dist-web`.

**Native**:

```
cargo run --release -p arena
```

Click to grab the mouse · `WASD` moves · `Space`/`Shift` **dashes**
(10 s cooldown, in your movement direction) · left-click fires · `R`
restarts · `C` toggles CRT mode · `[` `]` / `-` `=` / `9` `0` tune
glow / bloom / exposure live · `Esc` releases.

Survive the waves. Shards swarm, sentinels shoot, and from wave 3 the
shards shoot too — every wave their fire gets faster and harder. Pillars
are real cover for both sides. Every 10 kills a medkit spawns at the
center; kills come slower on later waves, so healing scales down with
difficulty on its own. Every 10th wave the arena empties for the
mini-boss: a split icosahedron whose crown rises and spins while bolt
rings radiate from its exposed core — planted while open, so the attack
is also your window. (`--wave 10` jumps straight to it.)

## How the rendering works

![the corridor demo](docs/corridor.png)

1. **Occluder pass** — all triangles, depth-only, pushed slightly away
   from the camera by a tuned depth bias.
2. **Line pass** — edges as screen-space quads (round caps, analytic AA,
   constant pixel width like a real beam), depth-tested against pass 1.
   Lines on a surface survive; lines behind it are eaten. Additive
   blending makes crossings glow hotter, and endpoint caps read as CRT
   beam-dwell dots for free.
3. **Runtime silhouettes** — smooth-surface edges store both face normals;
   an edge draws only when its faces disagree about facing the eye, so
   curved objects get contours that follow the camera.
4. **Phosphor post** — everything renders linear HDR, then a
   threshold-less mip-chain bloom, exposure soft-clip, and optional CRT
   barrel/chroma/vignette. Assets author only *relative* glow strengths;
   the engine owns final brightness with a hue-preserving glow dial.

Depth cueing (lines fade with distance), world-unit dashes, per-instance
flicker, and an original angular stroke font round out the look.

## Content pipeline

```
Blender (or tools/gen_*.py) ──glTF──▶ vex-convert ──.vec──▶ scene.ron ──▶ engine
```

`vex-convert` welds vertices, classifies every edge (boundary / crease /
material-boundary → always drawn; smooth → runtime silhouette candidate;
coplanar → dropped), and passes authored line art straight through —
Blender "loose edges" export as glTF `LINES` and become drawn decoration,
which is how floor spirals and wall panels are made. Material conventions:

| In Blender                          | In the engine                     |
|-------------------------------------|-----------------------------------|
| emissive color (else base color)    | stroke color                      |
| emissive strength (KHR extension)   | glow — how hard it blooms         |
| material name contains `dash`       | dashed stroke (world-spaced)      |
| material name contains `flicker`    | intensity flicker animation       |
| loose edges / `LINES` primitives    | always-drawn line art             |

The `.vec` format stores the welded vertices, classified edges (with
normals for silhouettes), and the invisible occluder mesh in one small
binary. The occluder mesh doubles as the collision mesh — walls that eat
lines also stop the player, enemies, bullets, and line-of-sight.

### Level editor

![the level editor](docs/editor.png)

For blocking out playable spaces without Blender, `cargo run -p editor --
mylevel.ron` opens an in-engine editor: fly around, place boxes /
cylinders / wedges / doorframes with live hue, saturation and glow
controls, and watch it through the real renderer — the preview *is* the
game's pipeline, bloom and all. `F5` saves the editable RON document,
`F6` exports a `.vec` through the same converter machinery as Blender
content, so exported levels get silhouettes, clean welded outlines, and
collision for free. `F1` shows the full key reference in-app.

### Animation clips

Rigid-transform animation lives in hand-editable `.anim.ron` files:
keyframed tracks over position, rotation, scale — and **intensity**, so
glow itself can be animated (the most vector-native channel there is).
Step/linear/smooth easing, loop/once/ping-pong playback. Preview a clip
on any model with hot-reload on save:

```
cargo run -p viewer -- assets/suzanne/suzanne.vec --anim assets/test/pulse-spin.anim.ron
```

Games sample a `Clip` into a `Pose` and compose it onto any transform;
clips know nothing about what they animate.

## Workspace

| Crate / example      | What it is                                          |
|----------------------|-----------------------------------------------------|
| `crates/vex-core`    | geometry, `.vec` format, frustum, stroke font       |
| `crates/vex-render`  | wgpu passes: occluders, lines, HDR bloom/CRT post   |
| `crates/vex-engine`  | window shell (native + web), input, cameras, RON scenes, capsule collision + raycasts |
| `crates/vex-convert` | glTF → `.vec` converter (CLI and library)           |
| `crates/vex-audio`   | 3D spatial audio (kira) + synth toolkit; games own their sound banks |
| `examples/01-cube`   | minimal pipeline: lines on black, fly camera        |
| `examples/02-viewer` | model viewer — drop in `.vec`/`.gltf`, hot-reloads on save |
| `examples/03-corridor` | FPS walkthrough of the reference aesthetic        |
| `examples/04-arena`  | the game                                            |
| `examples/05-editor` | in-engine level editor — primitives with hue/glow dials, exports `.vec` |
| `examples/06-soundlab` | sound sketchpad — RON recipes, hot-reloaded playback, prints paste-ready Rust |

Every example has a headless `--screenshot` mode (deterministic `--demo`
simulation in the arena) — the project was verified throughout by
rendering frames and looking at them.

## Documentation

Proper engine docs live in [`docs/`](docs/README.md): [getting
started](docs/getting-started.md), [architecture](docs/architecture.md),
the [rendering pipeline](docs/rendering.md), the [file
formats](docs/formats.md) (`.vec` / scene / anim specs), [building a
game](docs/building-a-game.md), and [web builds](docs/web.md). The
examples double as the tutorials; `cargo doc --workspace --no-deps
--open` gives the API reference.

## Building

Native needs Rust (edition 2024) and Vulkan-capable drivers:

```
cargo test --workspace          # 60+ unit tests
cargo run --release -p arena    # or: cube · viewer · corridor
```

Regenerate assets (no Blender required — the demo content is generated):

```
python3 tools/gen_arena.py && python3 tools/gen_corridor.py
cargo run -p vex-convert -- assets/arena/arena.gltf     # etc.
```

Web build needs the `wasm32-unknown-unknown` std (rustup target, or
`rust-wasm` on Arch — **must match the `rust` package version exactly**)
and `wasm-bindgen-cli` matching the version in `Cargo.lock`:

```
tools/build_web.sh              # → dist-web/, ready for any static host
tools/package_demo.sh           # → native Linux tarball in dist/
```

The arena binary embeds all its assets: `target/release/arena` runs from
anywhere, standalone. An `assets/` directory next to the executable (or
in the working directory) overrides the embedded copies, so packaged
builds stay moddable.

## For AI agents

`.claude/skills/vector3d-engine/SKILL.md` is the working agreement for
AI tools touching this repo: the verify loop, which contracts are
load-bearing, and the escalation ladder for when something fights back.
Humans may also enjoy it.

## Provenance

Designed and built over a handful of days as a human ↔ AI collaboration:
direction, playtesting, and taste by
[Sirpixelalittle](https://github.com/Sirpixelalittle); architecture and
implementation driven through Claude (Anthropic) in a continuous session —
including the design document ([DESIGN.md](DESIGN.md)), which was written
before the first line of code and still matches what shipped, milestone
by milestone.

Suzanne test model © Blender Foundation, via
[Khronos glTF-Sample-Assets](https://github.com/KhronosGroup/glTF-Sample-Assets)
(see `assets/suzanne/README.md`). The sword and pistol are original
Blender exports; everything else is generated by the scripts in `tools/`.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option — the standard Rust-ecosystem dual license. This covers
the engine, the games, the tools, and the generated/original assets
(Suzanne remains CC-BY 4.0, Blender Foundation, as noted above).

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
