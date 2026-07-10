# Windows builds

Cross-compiled from Linux with the GNU toolchain — no Windows machine
needed for the build itself.

## One-time setup

```sh
sudo pacman -S mingw-w64-gcc              # cross linker + Windows CRT
rustup target add x86_64-pc-windows-gnu   # Rust std for the target
```

The repo's `.cargo/config.toml` already points cargo at the
`x86_64-w64-mingw32-gcc` linker.

## Building

```sh
cargo build --release --target x86_64-pc-windows-gnu -p arena
# → target/x86_64-pc-windows-gnu/release/arena.exe
```

The exe is **self-contained**: all assets are embedded (`include_bytes!`,
with same-directory disk overrides for modding), and the mingw runtime is
statically linked — it imports only DLLs that ship with Windows 10+
(UCRT, DXGI, user32, …). Ship the single file.

Release builds hide the console window
(`windows_subsystem = "windows"`, gated to `not(debug_assertions)`);
debug builds keep it so `RUST_LOG` output stays visible.

## Smoke-testing under wine

Wine runs the exe, but its **builtin `d3dcompiler_47` cannot compile
wgpu's DX12 shaders** — wgpu 30 binds samplers through a Shader Model 5.1
descriptor heap (`SamplerState nagaSamplerHeap[2048]` with register
spaces), which wine's HLSL reimplementation rejects with a misleading
`E5008: Array size is not a positive integer constant`. That error is
wine's, not the engine's. Install Microsoft's real compiler first:

```sh
winetricks -q d3dcompiler_47
wine target/x86_64-pc-windows-gnu/release/arena.exe \
     --screenshot wine-test.png --demo 1 --size 640x360
```

With the real FXC in place the exe renders the deterministic demo scene
identically to the native build — same frame, different OS and graphics
API. A `create_factory_media failed` line in the log is a harmless wine
gap that wgpu works around.

Wine proves the binary; player-facing sign-off still wants a run on real
Windows hardware before shipping.

## Debugging shader translation

To see the HLSL that naga generates from a WGSL file (version-matched to
the workspace's wgpu):

```sh
cargo install naga-cli --version 30.0.0 --locked
naga crates/vex-render/src/shaders/post.wgsl /tmp/post.hlsl
```
