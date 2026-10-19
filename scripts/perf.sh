#!/usr/bin/env bash
# Measures what Sphere costs on this Mac: frame time with vsync off, CPU, GPU, energy and memory in normal use
# (vsync on, idle on the home screen) and startup. Plug the Mac in first: on battery the numbers drift.
# usage: scripts/perf.sh [label]        -> target/perf/<label>-<time>/summary.md (plus raw logs next to it)
#   BIN=target/dist/sphere-app scripts/perf.sh dist     measures another build
#   QUICK=1 scripts/perf.sh                             skips the profiles, the extra views and full screen
# macOS only: uses top, ioreg, sample and footprint (no sudo). Benches open a window for a few seconds.
set -uo pipefail
cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/sphere-app}
label=${1:-run}
OUT=${OUT:-target/perf/$label-$(date +%H%M%S)}
mkdir -p "$OUT"
S="$OUT/summary.md"
[ -x "$BIN" ] || { echo "no $BIN: build it first (cargo build --release -p sphere-app)"; exit 1; }
say() { echo "$*" | tee -a "$S"; }

# whole-GPU utilisation (all apps, WindowServer included) and GPU memory in use, from the accelerator's own counters
gpu_util() { ioreg -r -d 1 -w 0 -c IOAccelerator | grep -o '"Device Utilization %"=[0-9]*' | head -1 | cut -d= -f2; }
gpu_watch() { # seconds file
    for _ in $(seq "$1"); do gpu_util >>"$2"; sleep 1; done
}
avg() { awk '{ s += $1; n++ } END { if (n) printf "%.1f", s / n; else printf "-" }' "$1"; }
# top's per-process CPU %, energy impact and threads, one line a second; the first sample has no delta, dropped
cpu_watch() { # pid seconds file
    top -l "$(($2 + 1))" -s 1 -pid "$1" -stats pid,cpu,power,threads 2>/dev/null | awk -v p="$1" '$1 == p { print $2, $3, $4 }' | tail -n +2 >"$3"
}
col_avg() { awk -v c="$2" '{ s += $c; n++ } END { if (n) printf "%.1f", s / n; else printf "-" }' "$1"; }
# kills only what this script started
stop() { kill "$1" 2>/dev/null; sleep 0.5; kill -9 "$1" 2>/dev/null; wait "$1" 2>/dev/null; }
quiet_machine() { # other builds skew everything: wait (up to 10 min) for rustc/cargo elsewhere to finish
    for _ in $(seq 60); do
        busy=$(ps -Ao pcpu,comm | awk '$2 ~ /(rustc|cargo|ld|clang)$/ { s += $1 } END { printf "%.0f", s + 0 }')
        [ "${busy:-0}" -lt 30 ] && return
        echo "waiting for other builds to finish (${busy}% CPU in rustc/cargo/ld)"
        sleep 10
    done
}

say "# Sphere perf: $label"
say ""
say "- build: $(git rev-parse --short HEAD) $(git log -1 --format=%s | cut -c1-70), binary $BIN ($(du -h "$BIN" | cut -f1))"
say "- $(date '+%Y-%m-%d %H:%M'), $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.memsize | awk '{ printf "%d GB", $1 / 1073741824 }'), macOS $(sw_vers -productVersion)"
say "- power: $(pmset -g batt | sed -n 's/.*from //p' | head -1 | tr -d "'")$(pmset -g batt | grep -o '[0-9]*%' | head -1 | sed 's/^/ /')"
say "- display: $(system_profiler SPDisplaysDataType 2>/dev/null | awk -F': ' '/Resolution|Refresh Rate|UI Looks like|Main Display/ { printf "%s: %s; ", $1, $2 }' | sed 's/  */ /g')"
if pgrep -x sphere-app >/dev/null; then say "- note: another sphere-app is running (pid $(pgrep -x sphere-app | tr '\n' ' ')), numbers include its load"; fi
quiet_machine
say "- busiest processes before starting: $(ps -Ao pcpu,comm -r | sed -n '2,6p' | awk '{ n = split($2, p, "/"); printf "%s %s%%, ", p[n], $1 }')"
say ""

gpu_watch 5 "$OUT/gpu-idle.txt"
gpu_idle=$(avg "$OUT/gpu-idle.txt")

# 1. normal use: vsync on, the home screen, demo data (no service)
"$BIN" --demo >"$OUT/vsync.log" 2>&1 &
pid=$!
sleep 9
gpu_watch 15 "$OUT/gpu-vsync.txt" &
gw=$!
cpu_watch "$pid" 15 "$OUT/cpu-vsync.txt"
wait "$gw"
[ -z "${QUICK:-}" ] && sample "$pid" 5 -mayDie -file "$OUT/sample-vsync.txt" >/dev/null 2>&1
footprint "$pid" >"$OUT/footprint-vsync.txt" 2>&1
stop "$pid"
fp() { grep -o 'Footprint: [0-9.]* [KMG]B' "$1" | head -1 | cut -d' ' -f2-; }
gpu_mem() { awk '/IOAccelerator|IOSurface/ { v = $1; u = $2; if (u == "GB") v *= 1024; else if (u == "KB") v /= 1024; else if (u == "B") v /= 1048576; s += v } END { printf "%.0f MB", s }' "$1"; }

say "## Normal use (vsync on, home screen, 1600x1000 window)"
say ""
say "| | CPU % (100 = one core) | energy impact | GPU busy % (whole Mac) | memory footprint | GPU memory (IOAccelerator+IOSurface) |"
say "| --- | --- | --- | --- | --- | --- |"
say "| desktop without Sphere | | | $gpu_idle | | |"
say "| Sphere visible | $(col_avg "$OUT/cpu-vsync.txt" 1) | $(col_avg "$OUT/cpu-vsync.txt" 2) | $(avg "$OUT/gpu-vsync.txt") | $(fp "$OUT/footprint-vsync.txt") | $(gpu_mem "$OUT/footprint-vsync.txt") |"
say ""

# 2. frame cost: vsync off, so frames run as fast as the slowest of main thread, render thread and GPU
bench() { # name, then env/args
    local name=$1
    shift
    quiet_machine
    local line
    line=$(env "$@" 2>"$OUT/bench-$name.err" | grep '^bench:' | tail -1)
    say "| $name | ${line#bench: } |"
}
say "## Frame cost (vsync off, \`--bench 8\`, demo data)"
say ""
say "| view | result |"
say "| --- | --- |"
bench home "$BIN" --bench 8
bench home-2 "$BIN" --bench 8
bench home-3 "$BIN" --bench 8
if [ -z "${QUICK:-}" ]; then
    bench core-open "$BIN" --bench 8 --apps
    bench working "$BIN" --bench 8 --work
    bench swarm "$BIN" --bench 8 --swarm
    bench code "$BIN" --bench 8 --code
    bench git "$BIN" --bench 8 --git
    bench fullscreen SPHERE_FS=1 "$BIN" --bench 8
    bench fullscreen-2 SPHERE_FS=1 "$BIN" --bench 8
    bench offscreen-3200x2000 "$BIN" --bench 8 --offscreen --size 3200x2000
fi
say ""

if [ -z "${QUICK:-}" ]; then
    # where the time goes at full speed: CPU per thread while benching
    quiet_machine
    "$BIN" --bench 16 >"$OUT/bench-profile.log" 2>&1 &
    pid=$!
    sleep 6
    gpu_watch 6 "$OUT/gpu-bench.txt" &
    gw=$!
    cpu_watch "$pid" 4 "$OUT/cpu-bench.txt"
    sample "$pid" 4 -mayDie -file "$OUT/sample-bench.txt" >/dev/null 2>&1
    wait "$gw"
    wait "$pid" 2>/dev/null
    say "## While benching (vsync off): CPU $(col_avg "$OUT/cpu-bench.txt" 1)%, GPU busy $(avg "$OUT/gpu-bench.txt")%"
    say ""
fi

# 3. startup: three warm launches, SPHERE_STATS=1
say "## Startup (warm, SPHERE_STATS=1)"
say ""
for i in 1 2 3; do
    quiet_machine
    SPHERE_STATS=1 "$BIN" --demo >"$OUT/startup-$i.log" 2>&1 &
    pid=$!
    sleep 8
    stop "$pid"
    say "- run $i: $(grep -E 'first frame|third frame|boot animation done' "$OUT/startup-$i.log" | sed 's/stats: //' | tr '\n' ';' | sed 's/;$//')"
done
say "- $(grep -h 'stats: entities' "$OUT"/startup-*.log | head -1 | sed 's/stats: //')"
say ""

# thread totals from the profiles: samples per thread (1 ms each), busiest first
for f in "$OUT"/sample-*.txt; do
    [ -f "$f" ] || continue
    say "## $(basename "$f" .txt): busiest threads (samples, 1 ms each) and top-of-stack functions"
    say ""
    say '```'
    grep -E '^ {4}[0-9]+ Thread_' "$f" | sort -rn | head -8 | sed 's/^ *//' | cut -c1-140 | tee -a "$S"
    say ""
    awk '/^Sort by top of stack/ { on = 1; next } on && NF { print }' "$f" | head -35 | cut -c1-140 | tee -a "$S"
    say '```'
    say ""
done
echo "wrote $S"
