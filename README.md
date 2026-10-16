# Sphere OS, M0 look test

A small native Rust/Bevy program that checks the approved HUD look (the web prototype, Balanced theme) can be matched natively: the ring-deck core with bloom and a Depth view, a glass Agents panel and a live terminal panel, both rendered from Bevy UI to a texture and shown on tilted 3D quads.

## Run the look test

    cargo run --release

Keys: `D` toggles the Depth view, `T` focuses the terminal (or click it), `Esc` releases it. While focused, the keyboard goes to the shell.

Flags:

- `--cmd "<command>"` what the terminal runs first (default: `git log --graph --color --oneline -20`, then an interactive prompt)
- `--bench <seconds>` log fps and p50/p99 frame time, then exit (needs a window)
- `--shot <png> [--frames N] [--depth] [--term]` render N frames offscreen, save a PNG, exit (no window needed)
- `--size WxH`, `--no-msaa`

The window is 1600x1000. The frame-time overlay (fps, p50, p99) is top left.

Linux: needs X11 dev libs by default (`--features wayland` for Wayland, needs libwayland). macOS: `cargo run --release` should work as is (Metal), untested. If your rustc is slightly older than a dependency requires, add `--ignore-rust-version`.

No GPU (CI, containers): `scripts/shot-swiftshader.sh --shot look/out.png --frames 120 --no-msaa` uses a software Vulkan driver. Frame rates there mean nothing.

Images in `look/`: `compare.png` (prototype left, Rust right), `rust-flat.png`, `rust-depth.png`, `rust-terminal.png`.

## Credits

- [Bevy](https://bevy.org)
- [Alacritty](https://github.com/alacritty/alacritty) (`alacritty_terminal` for VT state)
- `portable-pty` from [WezTerm](https://github.com/wezterm/wezterm)
- Fonts under the SIL OFL (licences in `assets/fonts`): Michroma, Rajdhani, JetBrains Mono
