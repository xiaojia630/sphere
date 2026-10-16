# Sphere OS

An operating surface for a future where agents do most of the work and people
steer: no windows or app icons, just one sphere you can rotate, zoom into and
expand, wrapped in a JARVIS-style HUD.

This is step 1, the sphere itself. Click the core to expand it into your
spaces (Agents, Mail, Calendar, Files, Notes, Web), click a space to fly into
it and see what's inside, and press Esc or right-click to step back. The
spaces hold placeholder items for now; step 2 connects a Claude agent that
fills them with real tasks, and step 3 adds voice.

Built in Rust with [Bevy](https://bevyengine.org). The same code runs as a
desktop app and in the browser.

## Controls

| Input | Action |
| --- | --- |
| Drag (or one finger) | Rotate |
| Scroll (or pinch) | Zoom |
| Click (or tap) | Expand the core, enter a space, select an item |
| Esc, Backspace or right-click | Go back one level |

## Run it on your desktop

Install Rust from <https://rustup.rs>, then:

```sh
cargo run --release
```

On Linux you also need the usual Bevy system packages (`libx11-dev`,
`libasound2-dev`, `libudev-dev`, and a Vulkan driver).

## Build the browser version

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129   # must match Cargo.lock
./build-web.sh                                     # uses wasm-opt if installed
python3 -m http.server -d web 8000                 # then open localhost:8000
```
