# Building a game

The contract: implement `App`, hand it to `run`, and each frame turn your
state into segments. The arena (`examples/04-arena`) is the full-scale
reference for everything on this page.

## The App trait

```rust
use vex_engine::{App, Input, run};

impl App for MyGame {
    fn init(&mut self, gpu: &Gpu, target_format: wgpu::TextureFormat) {
        // create renderers here — the GPU exists now, and on the web it
        // arrives asynchronously after run() returns
    }
    fn update(&mut self, dt: f32, input: &Input) { /* simulate */ }
    fn render(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder,
              color: &wgpu::TextureView, depth: &wgpu::TextureView,
              viewport: Vec2) { /* record the frame recipe */ }
}

fn main() -> anyhow::Result<()> {
    run("my game", MyGame::new()?)
}
```

`run` owns the window, the event loop, mouse capture (click captures,
Escape releases), resize, and a live fps title. Render into the post
processor's `hdr_view()` and let `PostProcessor::run` composite to
`color` — see the frame recipe in [Rendering](rendering.md).

## Input

Polling, edge-aware, capture-aware:

```rust
input.is_down(KeyCode::KeyW)            // held
input.is_just_pressed(KeyCode::Space)   // this frame only
input.is_mouse_down(MouseButton::Left)
input.mouse_delta()                     // look, only while captured
input.scroll_delta()
input.is_captured()
```

`KeyCode` and `MouseButton` are re-exported from winit — no direct winit
dependency needed.

## Cameras and controllers

- **`FpsController`** — a walking player: capsule (`radius`, `height`,
  `eye_height`), gravity, optional jump/sprint, optional dash
  (`dash_enabled`, `dash_cooldown`, `dash_speed`, `dash_decay`, with
  `dash_ready_fraction()` for HUD meters and `just_dashed()` for sfx).
  `update(dt, input, &soup)` does all movement through collision.
- **`FlyCamera`** — free flight for tools: `planar_movement` keeps
  forward/back level with the ground (editor-style), `turn_speed` drives
  arrow-key yaw/pitch. Provides `view()` / `view_proj(aspect)`.
- **`OrbitCamera`** — turntable for viewers.

## Collision

Build a `TriangleSoup` from your world geometry (typically the baked
scene's occluder mesh — the same triangles that hide lines stop bodies):

```rust
let soup = TriangleSoup::new(&vertices, &indices, 2.0 /* grid cell */);
soup.raycast(origin, dir, max_dist)      // Option<f32> hit distance
soup.line_of_sight(a, b)                 // bool
slide_capsule(&soup, feet, radius, height, motion)  // -> { position, grounded }
```

**The law: nothing teleports.** Every position change — movement, AI
steering, knockback, separation pushes — goes through `slide_capsule`. A
raw `pos +=` can cross a wall plane and leave a body permanently outside
the world (this shipped a soft-lock once; see
[Architecture](architecture.md)).

## Sim / render split

Keep game rules in a module with **no GPU types** (`game.rs` in the
arena): plain state in, events out, exhaustively unit-testable. The
binary crate turns that state into `Vec<Segment>` each frame — posed
models via `edge_segments` / `silhouette_segments` + a transform,
procedural effects (projectiles, muzzle flashes, HUD text via
`vex_core::font`) built segment by segment. Dynamic occluders work the
same way: transform vertices, append triangles.

Communicate outward with an event queue the shell drains once per frame
(the arena's `GameEvent` → audio mapping), not with callbacks.

## Animation

Author clips as `.anim.ron` ([format](formats.md)), then:

```rust
let clip = Clip::load("swing.anim.ron")?;
let mut player = AnimPlayer::new(clip);
player.update(dt);
let pose = player.pose();               // -> Pose
let matrix = pose.transform();          // T·R(YXZ)·S — pose segments AND occluders
```

Use clips for *authored* motion on objects (doors, props, boss
choreography previews). Motion derived from sim state (a projectile's
roll, recoil, hover wobble) is clearer as code.

## Audio

The engine plays; the game owns the bank
(`examples/04-arena/src/sounds.rs` is the pattern):

```rust
let sounds = Sounds::synth();                  // your struct, your recipes
let mut audio = AudioEngine::new()?;           // after a user gesture (web!)
audio.set_listener(eye, orientation);          // every frame
audio.play(&sounds.shot);                      // non-spatial
audio.play_at(&sounds.impact, world_pos);      // panned + attenuated (2–42 m)
```

Recipes compose `vex_audio::synth` primitives (`sweep`, `sweep_exp`,
`burst`, `mix`, `append`, `to_sound`, …) — deterministic, no audio files.
Adding a sound never touches engine code.

Prototype recipes with the sound lab before committing to one:
`cargo run -p soundlab -- sketch.sound.ron` bootstraps a documented
sketch, replays it on every save, and prints the paste-ready Rust
(`c` in the tool, or `--code`).

## Assets and shipping

- Load content from an `assets/` directory next to the executable;
  embed it with `include_bytes!` as the fallback (the arena's `embedded`
  module) so a bare binary still runs and the web build needs no fetches.
- `tools/package_demo.sh` produces the shippable layout (and
  `--windows` cross-builds a zip); [Web builds](web.md) covers wasm.

## Verification loop

Two habits carry this codebase:

1. **Pure sim tests** for rules (`cargo test`), including deterministic
   soak tests for intermittent bugs (fixed-seed RNG makes replays exact).
2. **Headless screenshots** for anything visual: add `--screenshot`
   plumbing early (`Gpu::headless()` + `HeadlessTarget`), render a posed
   frame, and look at the PNG before calling a feature done.
