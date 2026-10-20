#!/usr/bin/env bash
# Builds the web playground: the app's demo running in a browser (WebGPU), with sample agents, repos, containers
# and ports, a pretend shell, and the app's own sounds. Output: target/web/ (index.html, the wasm, gzipped, and its
# JS glue), ready for any static host.
#
#   scripts/build-web.sh            then serve target/web, e.g. python3 -m http.server -d target/web
#
# Needs the wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown), wasm-bindgen-cli at the
# version in Cargo.lock (cargo install wasm-bindgen-cli --version <it>), wasm-opt from binaryen if it's on PATH, and ffmpeg for the sounds.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
out=target/web
vendor=target/web-vendor
profile=${SPHERE_WEB_PROFILE:-web}
# the patch below rewrites Cargo.lock's entry for alacritty_terminal; put the lock back afterwards
mkdir -p "$vendor" && cp Cargo.lock "$vendor/Cargo.lock.orig"
trap 'cp "$vendor/Cargo.lock.orig" Cargo.lock' EXIT

# alacritty_terminal's emulator runs anywhere, but its PTY half (tty, event_loop and their polling crate) doesn't
# build for wasm (nor does the home crate that half uses). The build uses a copy of it without that half; the app never uses it there.
src=$(cargo metadata --format-version 1 | python3 -c 'import json,sys; print(next(p["manifest_path"] for p in json.load(sys.stdin)["packages"] if p["name"]=="alacritty_terminal"))')
rm -rf "$vendor/alacritty_terminal" && cp -R "$(dirname "$src")" "$vendor/alacritty_terminal"
python3 - "$vendor/alacritty_terminal" <<'PY'
import sys, re
d = sys.argv[1]
lib = open(f"{d}/src/lib.rs").read()
for m in ("event_loop", "tty"):
    lib = lib.replace(f"pub mod {m};", f'#[cfg(not(target_arch = "wasm32"))]\npub mod {m};')
open(f"{d}/src/lib.rs", "w").write(lib)
toml = open(f"{d}/Cargo.toml").read()
for dep in ("polling", "home"):
    toml = toml.replace(f"[dependencies.{dep}]", f"[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies.{dep}]")
open(f"{d}/Cargo.toml", "w").write(toml)
PY

cargo build --profile "$profile" --target wasm32-unknown-unknown -p sphere-app ${CARGO_FLAGS:-} \
    --config "patch.crates-io.alacritty_terminal.path='$vendor/alacritty_terminal'"

rm -rf "$out" && mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" --out-name sphere "target/wasm32-unknown-unknown/$profile/sphere-app.wasm"
if command -v wasm-opt >/dev/null; then
    wasm-opt -O2 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
        "$out/sphere_bg.wasm" -o "$out/sphere_bg.opt.wasm" && mv "$out/sphere_bg.opt.wasm" "$out/sphere_bg.wasm"
fi
# Hosts that don't compress .wasm get it gzipped anyway: the page inflates it (DecompressionStream)
gzip -9 -c "$out/sphere_bg.wasm" > "$out/sphere_bg.wasm.gz" && rm "$out/sphere_bg.wasm"
cp web/index.html "$out/index.html"
# The sounds load beside the page once the visitor first clicks or types; MP3 plays in every browser
mkdir -p "$out/sounds"
for w in crates/sphere-app/assets/sounds/*.wav; do
  ffmpeg -loglevel error -y -i "$w" -ac 1 -b:a 96k "$out/sounds/$(basename "${w%.wav}").mp3"
done
ls -la "$out"
