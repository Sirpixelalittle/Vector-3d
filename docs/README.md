# vector3d documentation

Everything on this page is about the **engine** — the five `crates/`. The
`examples/` are deliberately *not* documented beyond this table: they are
the living tutorials, kept small and readable, and they exercise every
engine feature the docs describe. When a doc and an example disagree,
trust the example and file the doc bug.

## Guides

| Doc | What it covers |
|-----|----------------|
| [Getting started](getting-started.md) | Build, run the examples, controls, tests |
| [Architecture](architecture.md) | Crate map, data flow, the rules the engine lives by |
| [Rendering](rendering.md) | The vector-CRT pipeline: occluders, lines, bloom, post |
| [File formats](formats.md) | `.vec` binary spec, scene `.ron`, animation `.anim.ron` |
| [Building a game](building-a-game.md) | App loop, input, cameras, collision, audio, shipping |
| [Web builds](web.md) | wasm/WebGPU builds, toolchain pinning, itch.io packaging |
| [Windows builds](windows.md) | Cross-compiling from Linux, wine smoke-testing |

## API reference

Every public item carries rustdoc. For the browsable API reference:

```sh
cargo doc --workspace --no-deps --open
```

## The examples, as a reading order

| Example | Teaches |
|---------|---------|
| `01-cube` | The minimal `App`: one model, one camera, the two render passes |
| `02-viewer` | Asset tooling: `.vec`/glTF loading, hot reload, animation clips, headless screenshots |
| `03-corridor` | Scenes: `scene.ron`, instances/tints, collision walking, fog, a weapon viewmodel |
| `04-arena` | A complete game: sim/render split, enemies, projectiles, HUD, audio bank, embedded assets |
| `05-editor` | The engine as a tool platform: live baking through `vex-convert`, exporting playable `.vec` |
| `06-soundlab` | The synth toolkit as a sketchpad: RON sound recipes, hot-reload playback, paste-ready Rust output |

Run any of them with `cargo run -p <name>` (`viewer` wants a model path —
see [Getting started](getting-started.md)).
