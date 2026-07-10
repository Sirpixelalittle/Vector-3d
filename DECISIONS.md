# Decisions & lessons

`DESIGN.md` records what was built. This file records what was *decided* —
including proposals that were evaluated and turned down — so future work
(human or agent) doesn't re-litigate settled questions or "fix" things that
are the way they are on purpose. If code looks odd, check here and the git
log before improving it.

Add new entries with a date. Reversing a decision is fine — but it happens
in a conversation with the maintainer, not silently in a diff.

## Physics: nothing teleports

Every position change of a collidable entity goes through
`vex_engine::collide::slide_capsule` — walking, dashing, enemy steering,
separation shoves, melee knockback, gun knockback. No raw `pos +=`, ever.

The rule exists because we shipped its violation three times. The slide
breaks contact ties using the pre-move capsule midpoint (the "came-from"
side), so a raw push that carries a capsule's center past a wall plane gets
resolved onto the far side *and kept there forever*. An enemy pinned outside
the arena is unkillable (bullets splash on the wall first) and the wave
never clears — a soft-lock. Strikes: spawn rings without body-width wall
clearance, then pairwise separation pushes, then the gun knockback
(a 0.6 shove vs a shard's 0.55 radius, caught by audit 2026-07-10). The
class of bug is closed exactly as long as the rule holds.

Corollary (2026-07-05): intermittent sim bugs get a deterministic soak test
before any fix — see `waves_never_soft_lock` (tens of thousands of frames,
mixed dts, watchdog asserts). It reproduced in 0.2 s what playtesting hit
sporadically.

## The look is tuned, not defaulted

Glow, bloom, fog, and exposure values are art direction, arrived at through
viewing rounds, and deliberately kept on the **dim** side — readability and
eye comfort over spectacle. Proposals to "optimize" or re-balance these
values are not perf work and have already been declined (2026-07-09).
Same for the pistol viewmodel pose (`GUN_*`, `WEAPON_LENGTH 0.52`,
translation `(0.16, -0.20, -0.52)`) and the dash's 10 s cooldown: reached
through playtesting; changing them un-decides the maintainer's decisions.

## Performance: measure first, and what's already been weighed

Perf work starts with quantifying the cost — "this looks slow" is not
evidence. When a web player reports lag, first check the browser console's
`adapter:` line for SwiftShader (blocklisted GPU → CPU rendering) before
touching the renderer.

Evaluated and **declined** (2026-07-09), with the reasoning — don't
re-propose without new numbers:

- **Line-shader micro-optimizations** — the fragment budget lives in the
  post chain (3+ full-screen RGBA16F passes) vs roughly one screen's worth
  of 4 px line strips. Wrong target.
- **GPU-side model instancing** — saves tens of µs at ≤20 enemies, costs
  two new pipelines including the guarded `line.wgsl`. Revisit at 100+
  model-entities per frame, with a benchmark.
- **O(n²) enemy separation** — n ≤ 21 → 210 pairs. Not a cost.
- **Capping web canvas resolution / devicePixelRatio** — visual change on
  hiDPI screens; maintainer said no (2026-07-09).
- **Repo-wide `cargo fmt`** — never been run; a formatting commit touches
  every file including the human-approval ones and pollutes blame for zero
  behavior change.

Accepted perf idiom: append-style `*_into` buffer reuse (CPU geometry,
collision query scratch). Byte-identical output is the acceptance bar —
see the verification recipe below.

## Audit triage (2026-07-10)

An external code audit's findings were verified and triaged. Fixed: gun
knockback through the slide; controller reset on restart
(`FpsController::reset_motion`); `.vec` docs wording. **Deferred, with
reasons:**

- **Converter hardening** (malformed-glTF panics, silently skipped
  triangle strips/fans) — every asset is authored in-house by
  `tools/gen_*.py`; no third-party glTF enters the pipeline. Revisit if
  external model imports become a workflow.
- **Weld epsilon correctness** (quantization buckets vs true
  epsilon-distance) — technically right, but changing weld semantics can
  reclassify edges on every regenerated asset. Zero observed artifacts;
  churn without benefit.
- **`.vec` loader strictness** (required chunks, duplicate rejection) —
  guarded file, for a format only our own tools write.
- **Near-plane clipping of fog/dash attributes** in `line.wgsl` — real
  but cosmetic, only on segments crossing the camera plane; wants a
  visible repro before touching a guarded shader.
- **Game-over isn't a strict freeze-frame** — deliberate: a full freeze
  would pin the damage flash red and hang sparks mid-air. Known wart: an
  in-flight slug can still land (and score) in the sub-second after death.

## Architecture decisions

- **Sim/render split** (M4): `game.rs` is a pure, unit-tested simulation
  that emits `GameEvent`s; `main.rs` is presentation. New gameplay follows
  this shape — logic testable headless, rendering derived from state.
- **Audio is game-owned** (2026-07-07): `vex-audio` is content-free — it
  plays `&StaticSoundData` and exports the `synth` toolkit. Sound recipes
  live in each game (`examples/04-arena/src/sounds.rs`). New sound = recipe
  + `Sounds` field + event match arm, all game-side; never put content in
  the engine crate. Prototype with `cargo run -p soundlab -- x.sound.ron`.
- **Web pointer lock can be revoked silently** (2026-07-08): browsers exit
  pointer lock on Esc and *eat the keydown*. The shell detects revocation
  by the frozen-cursor invariant (3-event debounce). Any future
  capture-state feature must assume the browser can drop the lock without
  telling you.
- **Gameplay effects must not depend on audio** (2026-07-09): event
  side-effects (powerup grants, etc.) are applied before `play_events`,
  which early-returns when audio is off. Sound handlers stay sound-only.
- **HUD is sized in physical pixels** — known issue: small on hiDPI /
  fullscreen. Unfixed by choice so far; do not conflate with the declined
  render-scale cap above.

## Testing lore (gotchas that have bitten more than once)

- Controller/input tests must call `input.end_frame()` after each simulated
  frame, or a held key's "just pressed" edge re-triggers every frame.
- The slug spawns at the **muzzle**, so tests that move the eye must move
  the muzzle with it (the old hitscan didn't care; the projectile does).
- Boss tests: a young `age` puts the boss inside the spawn ramp and makes
  it untargetable. Keep `age` past the ramp and set
  `boss_cycle_start = Some(boss.age - 0.1)` to open the attack-cycle
  window instead.

## Verification recipe for refactors

For any change that claims "no visual/sim impact", prove it byte-for-byte:

```sh
# render the four standard scenes, before (stash) and after, then compare
cargo run -p arena -- --screenshot a.png --demo 5.05 --size 800x450
cargo run -p arena -- --screenshot b.png --wave 10 --demo 16.4 --pos 6,0,0 --yaw 90 --size 800x450
cargo run -p arena -- --screenshot c.png --menu --size 800x450
cargo run -p arena -- --screenshot d.png --powerup --size 800x450
```

Identical commands against the pre-change build (`git stash` → render →
`git stash pop` → render), then `cmp` the PNGs. All four identical =
proven. A hash mismatch is not necessarily your bug — check whether the
baseline predates an intentional change — but it always needs an
explanation before merging. The arena sim is deterministic (fixed-seed
RNG), which is what makes this work; keep it that way.

## Toolchain footnotes

The environment quirks live in the skill file (wasm LTO off,
wasm-bindgen-cli pinned to `Cargo.lock`). One addition worth its own
note: `wasm-opt` needs explicit `--enable-*` feature flags in
`tools/build_web.sh` because `strip = "symbols"` removes the
`target_features` section binaryen would otherwise read. Symptom of
removing them: "Fatal: error validating input" on perfectly valid wasm.

Switched from distro rust to rustup (2026-07-10), which ended the old
rust/rust-wasm exact-pkgrel matching requirement and enabled the Windows
cross-compile target (`docs/windows.md`). The wasm `lto="off"` workaround
predates the switch and may now be removable — verify in a browser before
touching it.
