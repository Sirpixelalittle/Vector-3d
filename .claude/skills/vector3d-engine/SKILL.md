---
name: vector3d-engine
description: How to work on the vector3d engine and games without breaking it — build/verify workflow, load-bearing contracts, safe-to-edit zones, and the stuck protocol. Use whenever writing or modifying code, shaders, or assets in this repository.
---

# Working on vector3d

A from-scratch vector-CRT engine (Rust + wgpu). Everything on screen is a
glowing stroke; triangles exist only to occlude strokes via the depth
buffer. Read `DESIGN.md` for architecture; this skill is about *how to
change things safely*.

## The prime directive

**When something doesn't work, the engine is almost never the thing to
change.** The engine is small, tested, and its quirks are deliberate. If
a feature fight leads you toward "rewriting how the renderer/collision/
format works", STOP — you are about to cleave a working machine. Present
your diagnosis to the user and ask, instead of mutating your way out.

## Verification loop (non-negotiable)

1. `cargo test --workspace` — 16+ suites, all green before and after.
2. `cargo clippy --workspace --all-targets` — stays at zero warnings.
3. **Render a screenshot and actually look at it.** Every example has
   `--screenshot out.png [--size WxH]`:
   - viewer: `MODEL --yaw --pitch --zoom [--anim clip --time T]`
   - corridor: `--pos x,y,z --yaw --pitch --crt --glow --bloom`
   - arena: `--demo SECS --recoil 0..1 --pack AGE --pos --yaw --pitch`
   - editor: `--level file.ron --pos --yaw --pitch`
4. Windowed smoke: `RUST_LOG=warn timeout 5 cargo run -q -p <example>`.
5. Web build must keep compiling if you touched shared code:
   `tools/build_web.sh`.

Visual changes are judged by rendered evidence, not by whether code
compiles. Gameplay/animation *feel* is judged by the user — expect
iteration, don't declare feel "fixed" from a static screenshot.

## Load-bearing contracts — do NOT change without explicit user approval

- **`.vec` binary format** (chunks PALT/VERT/EDGE/OCCL/AABB, style bits).
  Shipped assets and builds depend on it. Additions need a version
  strategy + converter + loader + regenerating every asset together.
- **Rust↔WGSL struct pairs**: `CameraUniform` (112 bytes) ↔
  `camera.wgsl`, `PostParams` ↔ `post.wgsl`. Field order, size, padding
  — change both sides together or corrupt every draw.
- **Render pass order and clear/load semantics** (see DESIGN.md):
  occluders clear depth with bias 2/2.0 (tuned against z-fighting by
  screenshot — not arbitrary); weapon layer *clears* depth (never clips
  walls); HUD clears depth; composite applies sRGB **iff** the target
  format is linear (that's the web path — removing it kills browser glow).
- **Edge classification semantics**: runtime edges are binary
  Always/Smooth; smooth edges carry both face normals for the
  silhouette test `(n1·v)(n2·v) ≤ 0` with the eye in model space.
- **Physics law: nothing teleports.** Every position change goes through
  `slide_capsule` — including knockbacks and separation pushes. A raw
  `pos +=` can cross a wall plane and the came-from tie-break will keep
  the entity on the wrong side forever (this shipped a soft-lock once).
  Spawn points need a body-width of clearance from walls.
- **User-approved feel constants** — these encode the user's taste,
  reached through playtesting rounds; changing them un-decides their
  decisions: pistol pose (`GUN_*`, `WEAPON_LENGTH`, translation
  `(0.16, -0.20, -0.52)`), glow defaults (kept dim — comfort over
  spectacle), muzzle effects live in the weapon layer only.
- **Tests are contracts.** Never delete/weaken a test, `#[ignore]` a
  failure, or loosen an assertion to get green. A failing test means the
  code is wrong or the world changed — find out which, report it.

## Post-cutoff dependencies — your training data is wrong here

wgpu 30, winit 0.30 (web), kira 0.12, glam 0.33 are newer than most
models' training data. The code as written **compiles and is correct**:
`queue.present()`, `CurrentSurfaceTexture` enum, Option-wrapped depth
state, `glam::camera::rh::proj::directx::*`, kira spatial tracks with
`persist_until_sounds_finish` — none of these are mistakes to "fix".
If an API surprises you, read the real source:
`~/.cargo/registry/src/*/CRATE-VERSION/src/` — grep it BEFORE writing
code against it. Never bump or downgrade a dependency to dodge a
compile error.

## Safe-to-edit zones

- `examples/*` game logic — freely, keeping their unit tests green.
  Pattern: pure sim module (`game.rs`, tested, emits events) +
  presentation (`main.rs`).
- Sounds: `crates/vex-audio/src/synth.rs` amp/freq numbers. All SFX are
  synthesized — never add audio files. Audible check:
  `cargo test -p vex-audio -- --ignored`.
- Assets: `tools/gen_*.py` → `cargo run -p vex-convert -- file.gltf`;
  scene RON; `.anim.ron` clips; editor levels. Sloppy overlapping-box
  meshes are fine — welding + coplanar-drop clean them up.
- Engine crates: **additive** changes (new modules, new pub fns, opt-in
  flags like `planar_movement`) are the house style. Changing existing
  behavior needs every caller checked and screenshots before/after.

## Environment quirks (documented, not bugs — don't "fix" them)

- `build_web.sh` forces `lto="off"` for wasm: CachyOS bakes
  x86-64-v3 into rust-wasm's std bitcode and thin-LTO re-codegens it.
  Leave it.
- Arch `rust` and `rust-wasm` must match to the exact pkgrel;
  wasm-bindgen-cli must match `Cargo.lock`. Version-skew errors look
  like thousands of `E0425 Ok/Err not found`.
- Browsers cache wasm hard — hard-refresh after rebuilds.
- Web audio legally starts only on a user gesture; the engine inits
  audio on the first captured click. Don't move it earlier.
- Stray `'x86-64-v3' is not a recognized processor` warnings elsewhere
  are cosmetic.

## When stuck — the escalation ladder

1. **Reproduce minimally**: a focused unit test or a headless
   screenshot. For intermittent sim bugs, write a deterministic soak
   test (see `waves_never_soft_lock` — 110k frames, mixed dts, found in
   0.2s what the user hit sporadically).
2. **Read the source** — this repo's, and the dependency's in the cargo
   registry. Not the docs you remember; the source on disk.
3. **Check `DESIGN.md` + git log** — the quirk you're fighting is
   probably documented with the reason it exists.
4. **Fix at the right layer, additively** — e.g. web glow was fixed by
   an `srgb_encode` flag in the post shader, not by rewriting surface
   selection.
5. **If the fix requires touching a load-bearing contract → stop and
   ask the user.** Bring the diagnosis, the failing evidence, and the
   options. That conversation costs a minute; an engine cleave costs
   days.

Commit style: `type: description` (feat/fix/refactor/docs/test/chore),
body explains why, **no Co-Authored-By footers**. Commit only when tests
+ clippy + screenshots pass.
