#!/usr/bin/env bash
# Builds the plugin and (optionally) copies it into a Pumpkin server's plugins folder.
#   ./build.sh                 -> builds only
#   ./build.sh /path/to/server -> builds and installs into /path/to/server/plugins
set -euo pipefail

rustup target add wasm32-wasip2 >/dev/null 2>&1 || true
cargo build --release --target wasm32-wasip2

WASM="target/wasm32-wasip2/release/pumpkin_essentials.wasm"
echo "Built: $WASM ($(du -h "$WASM" | cut -f1))"

if [[ $# -ge 1 ]]; then
  mkdir -p "$1/plugins"
  cp "$WASM" "$1/plugins/"
  echo "Installed into $1/plugins — restart the server."
fi
