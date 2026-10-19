#!/usr/bin/env bash
# Runs a command (usually sphere-app) inside the README clips' demo: a home of its own with a small Rust repo, its own
# sphered whose agent is fake-claude playing demo.py's script, and stand-in `docker` and `lsof` with a sample compose
# project. Your ~/.sphere, ~/.claude, Docker and sphered are never touched.
#
#   scripts/readme-demo/run.sh <demo dir> [--task] [--pace N] -- <command...>
#
# --task starts the agent on "Add a --json flag" once the command has been up for $TASK_AFTER seconds (default 3).
# --pace stretches the agent's pauses (recordings render slower than real time). Needs sphered, sphere-host,
# fake-claude and sphere in $SPHERE_BIN (default target/debug): cargo build -p sphered -p sphere-host -p fake-claude -p sphere-cli
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
demo=$(mkdir -p "$1" && cd "$1" && pwd); shift
task=0 pace=1
while [ $# -gt 0 ]; do
    case "$1" in
    --task) task=1 ;;
    --pace) pace=$2; shift ;;
    --) shift; break ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
    esac
    shift
done
bin=$(cd "${SPHERE_BIN:-$here/../../target/debug}" && pwd)

# the demo home replaces HOME, so keep the real toolchain reachable (the review runs cargo test)
export RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup} CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}
export HOME=$demo/home PATH=$here/bin:$PATH
repo=$(python3 "$here/demo.py" "$HOME" "$bin" "$pace")
# sockets need a short path
run=$(mktemp -d /tmp/sphere-demo.XXXX)
export XDG_RUNTIME_DIR=$run TMPDIR=$run
"$bin/sphered" > "$demo/sphered.log" 2>&1 &
daemon=$!
# sphere-host outlives sphered by design; this one is the demo's own (its socket is in $run)
stop() { kill $daemon 2>/dev/null; wait $daemon 2>/dev/null; pkill -f -- "--socket $run/" 2>/dev/null; rm -rf "$run"; }
trap stop EXIT
# Linux puts the socket under XDG_RUNTIME_DIR, macOS under the demo home (keep <demo dir> short there)
for _ in $(seq 50); do [ -S "$run/sphere/sphered.sock" ] || [ -S "$HOME/.sphere/run/sphered.sock" ] && break; sleep 0.1; done
if [ $task = 1 ]; then
    (sleep "${TASK_AFTER:-3}"; "$bin/sphere" run "$repo" "Add a --json flag" > "$demo/task.log" 2>&1) &
fi
"$@"
