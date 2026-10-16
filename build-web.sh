#!/usr/bin/env bash
# Builds the browser version into web/. Needs: rustup target wasm32-unknown-unknown,
# wasm-bindgen-cli (same version as the wasm-bindgen crate in Cargo.lock), and
# optionally wasm-opt from binaryen to shrink the output.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --target wasm32-unknown-unknown --profile wasm-release
wasm-bindgen --no-typescript --target web --out-dir web/pkg --out-name sphere_os \
  target/wasm32-unknown-unknown/wasm-release/sphere-os.wasm
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals --enable-reference-types --enable-multivalue \
    web/pkg/sphere_os_bg.wasm -o web/pkg/sphere_os_bg.wasm
fi
ls -lh web/pkg
