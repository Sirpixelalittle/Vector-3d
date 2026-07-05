#!/usr/bin/env bash
# Package the arena demo into shareable archives under dist/.
#
#   tools/package_demo.sh            # Linux tarball (always)
#   tools/package_demo.sh --windows  # also cross-build a Windows zip
#
# The archive layout is exactly what the binary expects: assets/ sits next
# to the executable (see asset_root() in examples/04-arena/src/main.rs).

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(date +%Y%m%d)
DIST=dist
ASSETS=(
    assets/arena/scene.ron
    assets/arena/arena.vec
    assets/arena/shard.vec
    assets/arena/sentinel.vec
    assets/pistol.vec
)

stage_assets() {
    local stage=$1
    for asset in "${ASSETS[@]}"; do
        mkdir -p "$stage/$(dirname "$asset")"
        cp "$asset" "$stage/$asset"
    done
    cat > "$stage/README.txt" <<'EOF'
VECTOR3D — ARENA          a neon wave shooter drawn entirely in glowing lines

RUNNING
  Linux:    ./vector3d-arena         (needs Vulkan drivers; any modern GPU)
  Windows:  vector3d-arena.exe

CONTROLS
  click        capture the mouse (Esc releases)
  mouse        look
  WASD         move        LShift  sprint        Space  jump
  left click   fire
  R            restart after game over
  C            CRT mode (barrel distortion + chroma)
  [ ]          glow   - =  bloom   9 0  exposure  (look tuning)

Survive the waves. Shards swarm, sentinels shoot — from wave 3 the shards
shoot too, and every wave their fire gets faster and harder. Pillars are
real cover: bolts splash on them, and so do your shots.
EOF
}

echo "==> building Linux release"
cargo build --release -p arena
LINUX_STAGE=$DIST/vector3d-arena-linux-x86_64
rm -rf "$LINUX_STAGE"
mkdir -p "$LINUX_STAGE"
cp target/release/arena "$LINUX_STAGE/vector3d-arena"
stage_assets "$LINUX_STAGE"
tar czf "$DIST/vector3d-arena-linux-x86_64-$VERSION.tar.gz" \
    -C "$DIST" vector3d-arena-linux-x86_64
echo "    $DIST/vector3d-arena-linux-x86_64-$VERSION.tar.gz"

if [[ "${1:-}" == "--windows" ]]; then
    echo "==> building Windows release (x86_64-pc-windows-gnu)"
    if ! command -v rustup >/dev/null || ! command -v x86_64-w64-mingw32-gcc >/dev/null; then
        echo "    needs the cross toolchain:  sudo pacman -S rustup mingw-w64-gcc" >&2
        echo "    (then: rustup default stable)" >&2
        exit 1
    fi
    rustup target add x86_64-pc-windows-gnu >/dev/null
    cargo build --release -p arena --target x86_64-pc-windows-gnu
    WIN_STAGE=$DIST/vector3d-arena-windows-x86_64
    rm -rf "$WIN_STAGE"
    mkdir -p "$WIN_STAGE"
    cp target/x86_64-pc-windows-gnu/release/arena.exe "$WIN_STAGE/vector3d-arena.exe"
    stage_assets "$WIN_STAGE"
    (cd "$DIST" && zip -qr "vector3d-arena-windows-x86_64-$VERSION.zip" \
        vector3d-arena-windows-x86_64)
    echo "    $DIST/vector3d-arena-windows-x86_64-$VERSION.zip"
fi

echo "==> done"
