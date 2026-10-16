# Sphere OS

A desktop for a future where agents do most of the work and people steer.
The core in the middle is the agent's presence: it breathes when nothing is
happening, spins up while agents work, and turns amber when something needs
you. Around it is a real desktop with workspaces, a dock, windows, live
widgets, an activity feed where you approve what agents produce, and a
command bar.

Built in Rust with [egui](https://github.com/emilk/egui). The same code runs
as a desktop app and in the browser.

## What's in it

- **Workspaces.** Work, Life and Build, each with its own windows.
- **The core.** Click it for a ring of every app. Its colour shows the
  agents' rolled-up state.
- **Activity feed.** Every agent task with its state (working, needs you,
  done) and progress. Results wait for Approve or Discard.
- **Apps for builders.** Agent chat, a code editor with a terminal pane and
  agent suggestions, Docker containers with logs, a music player, a shell,
  and a system monitor, plus Mail, Calendar, Notes, Files and Web.
- **Command bar.** Press `/`. App, music and container commands run directly
  (`open docker`, `next`, `restart api`); anything else becomes an agent task.

Agents, containers, music and metrics are simulated in this version. Each
service keeps the shape a real backend needs, so the real ones slot in
behind the same methods.

## Ideas taken from herdr

[herdr](https://github.com/herdrdev/herdr) is a terminal workspace manager
for coding agents. Sphere OS borrows its agent model: each agent is working,
blocked on you, or done, and those states roll up so one glance tells you
whether anything needs you. Next comes a herdr-style background service
(`sphered`) with a socket API that both the desktop and the browser connect
to, and rule files that detect the state of real agent terminals such as
Claude Code.

## Run it on your desktop

Install Rust from <https://rustup.rs>, then:

```sh
cargo run --release
```

## Build the browser version

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129   # must match Cargo.lock
./build-web.sh                                     # uses wasm-opt if installed
python3 -m http.server -d web 8000                 # then open localhost:8000
```

## Layout

| Path | What it holds |
| --- | --- |
| `src/app.rs` | The shell: top bar, dock, side panels, stage, windows, command bar |
| `src/core_view.rs` | The core and its app ring |
| `src/runtime.rs` | Agent tasks and their herdr-style states |
| `src/services.rs` | Music player, containers, metrics and toasts |
| `src/apps/` | One file per app |
| `src/theme.rs` | Colours, fonts and the HUD widgets |
| `src/icons.rs` | Line icons drawn on a 24-unit grid |

## Licence

Apache-2.0. The bundled fonts are under the SIL Open Font License; see
`assets/fonts`.
