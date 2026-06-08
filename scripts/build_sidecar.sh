#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON_DIR="$ROOT/src-python"
BINARIES_DIR="$ROOT/src-tauri/binaries"

echo ">>> Setting up Python environment with uv..."
cd "$PYTHON_DIR"
uv sync --dev

echo ">>> Building sidecar binary with PyInstaller..."
uv run pyinstaller \
    --onefile \
    --name python-sidecar \
    --distpath "$BINARIES_DIR" \
    --workpath "$PYTHON_DIR/build" \
    --specpath "$PYTHON_DIR" \
    --collect-all browser_use \
    --exclude-module oci \
    --exclude-module browser_use.llm.oci_raw \
    main.py

ARCH=$(uname -m)
OS=$(uname -s)

case "$OS" in
  Darwin)
    TARGET=$( [ "$ARCH" = "arm64" ] && echo "aarch64-apple-darwin" || echo "x86_64-apple-darwin" )
    ;;
  Linux)
    TARGET=$( [ "$ARCH" = "x86_64" ] && echo "x86_64-unknown-linux-gnu" || echo "aarch64-unknown-linux-gnu" )
    ;;
  MINGW*|MSYS*|CYGWIN*)
    TARGET="x86_64-pc-windows-msvc"
    BINARY_EXT=".exe"
    ;;
  *)
    echo "Unsupported OS: $OS" >&2; exit 1
    ;;
esac

SRC="$BINARIES_DIR/python-sidecar${BINARY_EXT:-}"
DST="$BINARIES_DIR/python-sidecar-${TARGET}${BINARY_EXT:-}"

echo ">>> Renaming to $DST"
mv "$SRC" "$DST"
echo ">>> Done: $DST"
