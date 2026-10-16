# Sphere

`sphered` is the background service behind Sphere. It starts and owns Claude Code agents (headless, over stream-json), gives each task its own git worktree and branch, records everything as one ordered event log in SQLite, and turns the agent's permission checks, questions and plans into requests that only you answer. Clients talk to it over a Unix socket with JSON-RPC 2.0; the `sphere` CLI is the first one.

This is milestone 1: the service, the process holder, the CLI and a fake agent for tests. The design is in [docs/sphered.md](docs/sphered.md). Desktop only (Linux and macOS).

## Crates

| Crate | What it is |
| --- | --- |
| `sphere-proto` | Events, requests, the JSON-RPC API types and the shared views. |
| `sphere-host` | A tiny detached process that owns agent processes and their output files, so `sphered` can restart without touching a running agent. |
| `sphered` | The service: store, hub, policy, workspaces, supervisor, API server. |
| `sphere-client` | Async client library. |
| `sphere-cli` | The `sphere` binary. |
| `fake-claude` | A scripted stand-in for `claude`, driven by the fixtures in `crates/fake-claude/fixtures`. Used by the tests. |

## Build and install

You need a recent stable Rust toolchain and git. To run real agents you need Claude Code installed and signed in.

```
cargo build --release
install -m755 target/release/{sphere,sphered,sphere-host} ~/.local/bin/
sphere doctor
```

`sphere doctor` checks Claude Code (version range and sign-in), git, the sandbox helpers (`bwrap`, `socat`) and whether sphered is running. Version and sign-in problems are warnings, never errors.

## The CLI

```
sphere daemon                       run sphered in the foreground
sphere run <repo> "<task>"          start an agent in a new worktree and branch (sphere/<slug>)
    --in-place --budget 2 --title T --mode plan --allow-rewrites --follow
sphere ls                           tasks, and what is waiting on you
sphere watch [task] [--typing]      follow live (a task is replayed from its start)
sphere answer <request> yes|change|later [text] [--always] [--input JSON]
sphere send <task> "<message>"      talk to the agent
sphere interrupt <task>             end the current turn
sphere stop <task>                  interrupt, then SIGTERM, then SIGKILL after a grace period
sphere mode <task> <mode>           change the permission mode
sphere model <task> <model>         change the model
sphere sessions                     Claude Code sessions on this machine
sphere bring-in <session>           continue a session as a Sphere task (refused while it runs elsewhere)
sphere doctor                       check the installation
```

`--socket` points the CLI at another sphered. By default the socket is in `$XDG_RUNTIME_DIR/sphere/` (else `~/.sphere/run/`).

### Answers

`yes` allows (optionally with an edited input; `--always` also saves the suggested rule), `change` refuses and tells the agent what to do instead, `later` snoozes the request. The first answer wins; any other gets `already_answered`. Stopping or interrupting a task withdraws its open requests. Answers are refused from any process an agent started, and from clients that identify as the assistant.

### Configuration

`~/.sphere/config.toml` (unknown keys are an error). The useful ones: `agent_command` (default `claude`), `max_concurrent` (3), `default_budget_usd` (5.0), `daily_cap_usd`, `stop_grace_ms`, `idle_ttl_ms` (idle agents stay warm for 10 minutes and hold no slot). The agent command is never read from a global environment variable.

### Policy

`~/.sphere/policy.toml` and a repo's `.sphere/policy.toml` are compiled into the agent's `--settings` (permission rules plus `sandbox.enabled`, on by default), plus `[[auto]]` rules that Sphere itself decides (for example `git push origin {branch}`). A repo policy can only add restrictions unless the user file says `trust_repos = true`. Force pushes and history rewrites are denied unless a task starts with `--allow-rewrites`.

A repo's `.sphere/config.toml` can list files to `copy` into each worktree and a `setup` command to run there.

## Tests

```
cargo test --workspace
cargo clippy --all-targets
```

The integration tests start the real `sphered` and `sphere-host` binaries on temporary directories with `fake-claude` as the agent, including killing sphered mid-run and restarting it.

### Live smoke test (ignored by default)

`crates/sphered/tests/live_claude.rs` runs one real agent with a five cent budget and a one-word prompt. It needs `claude` installed and signed in, and it spends a few cents, so it is never part of a normal run:

```
cargo test -p sphered --test live_claude -- --ignored --nocapture
```

Set `SPHERE_LIVE_CLAUDE` to the command if `claude` is not on your PATH.

## Credits

The idea of a small process that outlives the UI and owns the terminals comes from [herdr](https://github.com/herdrdev/herdr) (Apache-2.0). Sphere is built with its design in mind; no herdr code is used here.
