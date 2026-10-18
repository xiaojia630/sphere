# Contributing to Sphere

Thanks for helping. Sphere is a Rust workspace; the desktop app is `crates/sphere-app` (Bevy), the service is
`crates/sphered`, and `crates/fake-claude` stands in for Claude Code in tests.

## Getting started

```sh
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
target/release/sphere-app
```

## Before you open a pull request

- Tests and clippy pass. Tests stay offline: they use `fake-claude`, temporary repos and temporary folders, never a
  real agent, the network, or files outside a temp dir.
- Crates other than `sphere-app` follow `rustfmt` (`cargo fmt -p <crate>`). `sphere-app` keeps its own wide style:
  don't run `cargo fmt` on it, match the code around you instead.
- Processes run with argv only, never through a shell string.
- Nothing slow on the frame: anything that touches git, docker, the disk or the network runs on a background thread.
- New dependencies must be MIT or Apache-2.0.
- Commits follow [Conventional Commits](https://www.conventionalcommits.org) (`feat(sphere-app): …`,
  `fix(sphered): …`).
- UI changes: include a screenshot. `sphere-app --shot out.png --frames 200` renders one offscreen, and
  `--record <dir>` records frames for a GIF.

## Reporting bugs and ideas

Open an issue with what you did, what you expected and what happened, plus your macOS version and Mac. For security
problems, see [SECURITY.md](SECURITY.md) instead.

By contributing you agree that your contributions are licensed under Apache-2.0, and to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
