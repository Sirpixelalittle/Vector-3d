#!/usr/bin/env bash
# Build the arena demo for the browser (WebGPU) into dist-web/.
#
# Needs:
#   - the wasm32-unknown-unknown Rust std
#       rustup:  rustup target add wasm32-unknown-unknown
#       Arch:    sudo pacman -S rust-wasm
#   - wasm-bindgen CLI matching Cargo.lock:
#       cargo install wasm-bindgen-cli --version <see Cargo.lock> --locked
#
# Test locally (WebGPU needs localhost or https):
#   python3 -m http.server 8080 -d dist-web    →  http://localhost:8080
# Share by uploading dist-web/ to any static host (itch.io HTML project,
# GitHub Pages, Cloudflare Pages, …).

set -euo pipefail
cd "$(dirname "$0")/.."

LOCKED=$(grep -A1 'name = "wasm-bindgen"' Cargo.lock | grep version | head -1 | cut -d'"' -f2)
BINDGEN=${WASM_BINDGEN:-$(command -v wasm-bindgen || echo "$HOME/.cargo/bin/wasm-bindgen")}
if [[ ! -x "$BINDGEN" ]]; then
    echo "wasm-bindgen CLI not found. Install with:" >&2
    echo "  cargo install wasm-bindgen-cli --version $LOCKED --locked" >&2
    exit 1
fi

echo "==> cargo build (wasm32-unknown-unknown, release)"
cargo build --release -p arena --target wasm32-unknown-unknown

echo "==> wasm-bindgen (CLI $($BINDGEN --version | cut -d' ' -f2), lock $LOCKED)"
rm -rf dist-web
"$BINDGEN" --target web --no-typescript --out-dir dist-web --out-name arena \
    target/wasm32-unknown-unknown/release/arena.wasm
cp web/index.html dist-web/

SIZE=$(du -h dist-web/arena_bg.wasm | cut -f1)
echo "==> done: dist-web/ (wasm $SIZE)"
echo "    try it:  python3 -m http.server 8080 -d dist-web"
