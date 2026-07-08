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
# LTO off for wasm only: CachyOS bakes `target-cpu=x86-64-v3` into the
# rust-wasm std's bitcode, and cross-crate LTO re-codegens it, spamming
# "'x86-64-v3' is not a recognized processor" (harmless but noisy).
# Native builds keep the workspace thin-LTO profile.
cargo build --release -p arena --target wasm32-unknown-unknown \
    --config 'profile.release.lto="off"'

echo "==> wasm-bindgen (CLI $($BINDGEN --version | cut -d' ' -f2), lock $LOCKED)"
rm -rf dist-web
"$BINDGEN" --target web --no-typescript --out-dir dist-web --out-name arena \
    target/wasm32-unknown-unknown/release/arena.wasm
cp web/index.html dist-web/

# Optional but recommended: binaryen's whole-module optimizer. Recovers
# most of what the disabled wasm LTO leaves on the table (see above) and
# shrinks the download. Features must be listed explicitly: our release
# profile strips custom sections (`strip = "symbols"`), so binaryen can't
# read them from the module. All of these are baseline in every
# WebGPU-capable browser.
if command -v wasm-opt >/dev/null 2>&1; then
    RAW=$(du -h dist-web/arena_bg.wasm | cut -f1)
    echo "==> wasm-opt -O2 ($(wasm-opt --version))"
    wasm-opt -O2 \
        --enable-bulk-memory --enable-sign-ext --enable-mutable-globals \
        --enable-multivalue --enable-nontrapping-float-to-int \
        --enable-reference-types \
        dist-web/arena_bg.wasm -o dist-web/arena_bg.wasm.opt
    mv dist-web/arena_bg.wasm.opt dist-web/arena_bg.wasm
    echo "    $RAW → $(du -h dist-web/arena_bg.wasm | cut -f1)"
else
    echo "==> wasm-opt not found (pacman -S binaryen) — shipping unoptimized wasm"
fi

SIZE=$(du -h dist-web/arena_bg.wasm | cut -f1)
echo "==> done: dist-web/ (wasm $SIZE)"
echo "    try it:  python3 -m http.server 8080 -d dist-web"
