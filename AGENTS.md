# Agent guide

vector3d is a from-scratch vector-CRT 3D engine (Rust + wgpu): everything
on screen is a glowing stroke, and triangles exist only to occlude strokes.
Multiple coding agents work on this repository; this file is the shared
entry point for all of them.

Read in this order before changing code:

1. **`.agents/skills/vector3d-engine/SKILL.md`** — the working contract:
   verification loop, load-bearing contracts, the files that need human
   approval, post-cutoff dependency warnings, and the stuck protocol.
   (Claude Code loads its identical copy from `.claude/skills/`; the two
   are kept byte-for-byte in sync — any edit updates both.)
2. **`DECISIONS.md`** — the why-log: settled decisions with their reasons,
   including proposals already evaluated and declined. Check it before
   proposing an optimization or "fixing" something that looks odd.
3. **`DESIGN.md`** — what exists: architecture and the milestone log.
4. **`docs/`** — engine guides (getting started, rendering pipeline,
   formats, building a game, web builds).

House rules, in brief (the skill has the details):

- Four files require **explicit human approval before any edit**:
  `crates/vex-core/src/model.rs`, `crates/vex-render/src/shaders/*.wgsl`,
  `crates/vex-render/src/camera.rs`, `crates/vex-engine/src/collide.rs`.
  Present your diagnosis and the exact diff, then wait.
- **Nothing teleports**: every position change of a collidable entity goes
  through `slide_capsule`. No raw `pos +=`.
- Before any commit: `cargo test --workspace` green, `cargo clippy
  --workspace --all-targets` at zero warnings. Visual changes are judged
  by rendering screenshots and looking at them, not by compilation.
- wgpu 30, winit 0.30 (web), kira 0.12, glam 0.33 are newer than most
  models' training data. The code as written is correct — read the
  vendored source in `~/.cargo/registry/src/` before "fixing" an API you
  don't recognize, and never change a dependency version to dodge an
  error.
- Look/feel constants (glow levels, the pistol pose, dash timing) encode
  the maintainer's playtested taste — not tunables. See `DECISIONS.md`.
- Commits: `type: description` (feat/fix/refactor/docs/test/chore/perf),
  body explains why, **no AI attribution footers**. The maintainer pushes
  to GitHub themselves — never push.
