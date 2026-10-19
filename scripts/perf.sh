#!/usr/bin/env bash
# Measures what Sphere costs on this Mac: CPU (per thread, energy, wake-ups), GPU time and memory in normal use
# (vsync on, idle on the home screen), what a few settings change, frame time with vsync off, startup, and what the
# live system figures cost. Plug the Mac in first: on battery the numbers drift.
# usage: scripts/perf.sh [label]        -> target/perf/<label>-<time>/summary.md (plus raw logs next to it)
#   BIN=target/dist/sphere-app scripts/perf.sh dist     measures another build
#   QUICK=1 scripts/perf.sh                             skips the profiles, the what-ifs, the extra views and full screen
# macOS only: uses top, ps, ioreg, sample and footprint (no sudo). Each run opens a Sphere window for a few seconds.
# It never touches a Sphere that was already running, only notes what it uses.
set -uo pipefail
cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/sphere-app}
label=${1:-run}
OUT=${OUT:-target/perf/$label-$(date +%H%M%S)}
mkdir -p "$OUT"
S="$OUT/summary.md"
[ -x "$BIN" ] || { echo "no $BIN: build it first (cargo build --release -p sphere-app)"; exit 1; }
say() { echo "$*" | tee -a "$S"; }

# whole-GPU utilisation (all apps, WindowServer included), from the accelerator's own counters
gpu_util() { ioreg -r -d 1 -w 0 -c IOAccelerator | grep -o '"Device Utilization %"=[0-9]*' | head -1 | cut -d= -f2; }
gpu_watch() { # seconds file
    for _ in $(seq "$1"); do gpu_util >>"$2"; sleep 1; done
}
# GPU time (ns) one process has used so far: the GPU driver keeps it per client ("accumulatedGPUTime")
gpu_ns() { # pid
    ioreg -r -c IOUserClient -l -w 0 2>/dev/null | awk -v p="\"pid $1," '
        /\+-o / { if (mine) s += t; mine = 0; t = 0 }
        index($0, "\"IOUserClientCreator\" = " p) { mine = 1 }
        /accumulatedGPUTime/ { l = $0; while (match(l, /"accumulatedGPUTime"=[0-9]+/)) { t += substr(l, RSTART + 21, RLENGTH - 21); l = substr(l, RSTART + RLENGTH) } }
        END { if (mine) s += t; printf "%.0f", s }'
}
# GPU busy % of one process between two gpu_ns readings taken `secs` apart
gpu_pct() { awk -v a="$1" -v b="$2" -v s="$3" 'BEGIN { if (b > a) printf "%.1f", (b - a) / 1e7 / s; else printf "-" }'; }
avg() { awk '{ s += $1; n++ } END { if (n) printf "%.1f", s / n; else printf "-" }' "$1"; }
# top's per-process CPU % (100 = one core, as Activity Monitor shows it) and threads, one line a second
cpu_watch() { # pid seconds file
    top -l "$(($2 + 1))" -s 1 -pid "$1" -stats pid,cpu,threads 2>/dev/null | awk -v p="$1" '$1 == p { print $2, $3 }' | tail -n +2 >"$3"
}
fp() { grep -o 'Footprint: [0-9.]* [KMG]B' "$1" | head -1 | cut -d' ' -f2-; }
# GPU memory: the footprint's "(graphics)" rows (textures and buffers the GPU driver holds for the process)
fp_gfx() { awk '/\(graphics\)/ { v = $1; u = $2; if (u == "GB") v *= 1024; else if (u == "KB") v /= 1024; else if (u == "B") v /= 1048576; s += v } END { printf "%.0f MB", s }' "$1"; }
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
others() { ps -Ao pid,comm | awk '$2 ~ /sphere-app$/ { print $1 }'; }
load_note() { # what else is busy right now, for the record
    ps -Ao pcpu,comm -r | sed -n '2,5p' | awk '{ n = split($2, p, "/"); printf "%s %s%%, ", p[n], $1 }'
}

say "# Sphere perf: $label"
say ""
say "- build: $(git rev-parse --short HEAD) $(git log -1 --format=%s | cut -c1-70), binary $BIN ($(du -h "$BIN" | cut -f1))"
say "- $(date '+%Y-%m-%d %H:%M'), $(sysctl -n machdep.cpu.brand_string) ($(sysctl -n hw.perflevel0.physicalcpu 2>/dev/null) performance + $(sysctl -n hw.perflevel1.physicalcpu 2>/dev/null) efficiency cores), $(sysctl -n hw.memsize | awk '{ printf "%d GB", $1 / 1073741824 }'), macOS $(sw_vers -productVersion)"
say "- power: $(pmset -g batt | sed -n 's/.*from //p' | head -1 | tr -d "'")$(pmset -g batt | grep -o '[0-9]*%' | head -1 | sed 's/^/ /')"
say "- displays: $(system_profiler SPDisplaysDataType 2>/dev/null | awk -F': ' '/^        [^ ].*:$/ { printf "%s ", $1 } /Resolution|Refresh Rate|UI Looks like|Main Display/ { printf "%s: %s; ", $1, $2 }' | sed 's/  */ /g')"
say ""

# a Sphere that was already running: what it uses as it is, read only
pre=$(others)
if [ -n "$pre" ]; then
    say "## Sphere already running (not touched, only observed)"
    say ""
    say "| pid | running for | binary | CPU % | threads | GPU busy % (this process) | memory footprint | GPU memory |"
    say "| --- | --- | --- | --- | --- | --- | --- | --- |"
    for p in $pre; do
        g0=$(gpu_ns "$p")
        cpu_watch "$p" 6 "$OUT/cpu-pre-$p.txt"
        g1=$(gpu_ns "$p")
        footprint "$p" >"$OUT/footprint-pre-$p.txt" 2>&1
        say "| $p | $(ps -o etime= -p "$p" | tr -d ' ') | $(ps -o comm= -p "$p" | sed "s|$HOME|~|" | cut -c1-60) | $(avg "$OUT/cpu-pre-$p.txt") | $(tail -1 "$OUT/cpu-pre-$p.txt" | cut -d' ' -f2) | $(gpu_pct "$g0" "$g1" 6) | $(fp "$OUT/footprint-pre-$p.txt") | $(fp_gfx "$OUT/footprint-pre-$p.txt") |"
    done
    say ""
    say "Every number below was taken with that Sphere still running beside ours (it shares the CPU and the GPU)."
    say ""
fi
quiet_machine
say "- busiest processes before starting: $(load_note)"
say ""

gpu_watch 5 "$OUT/gpu-idle.txt"

# one vsync run with SPHERE_STATS=1: the app reports its own CPU from 8 s to 18 s (per thread, energy, wake-ups);
# meanwhile top (CPU % as Activity Monitor shows it), the GPU driver (this process's GPU time) and the whole GPU
vsync_run() { # name, then env/args
    local name=$1
    shift
    quiet_machine
    local note
    note=$(load_note)
    env SPHERE_STATS=1 "$@" >"$OUT/$name.log" 2>&1 &
    local pid=$!
    sleep 8.3
    local g0
    g0=$(gpu_ns "$pid")
    gpu_watch 9 "$OUT/gpu-$name.txt" &
    local gw=$!
    cpu_watch "$pid" 9 "$OUT/cpu-$name.txt"
    wait "$gw"
    local g1
    g1=$(gpu_ns "$pid")
    sleep 1.5
    [ "$name" = home ] && ioreg -r -c IOUserClient -l -w 0 2>/dev/null | grep -B3 -A14 "\"pid $pid," | head -60 >"$OUT/gpu-client-$name.txt"
    [ "$name" = home ] && [ -z "${QUICK:-}" ] && sample "$pid" 5 -mayDie -file "$OUT/sample-vsync.txt" >/dev/null 2>&1
    footprint "$pid" >"$OUT/footprint-$name.txt" 2>&1
    stop "$pid"
    local usage
    usage=$(grep -h 'stats: usage' "$OUT/$name.log" | tail -1 | sed 's/^stats: usage, //')
    say "| $name | $(avg "$OUT/cpu-$name.txt") | $(gpu_pct "$g0" "$g1" 9) | $(avg "$OUT/gpu-$name.txt") | $(fp "$OUT/footprint-$name.txt") | $(fp_gfx "$OUT/footprint-$name.txt") | ${usage:-no usage line, see $name.log} |"
    echo "  (busy before $name: $note)" >>"$OUT/load-notes.txt"
}

say "## Normal use (vsync on, home screen, demo data)"
say ""
say "Desktop without Sphere: whole GPU busy $(avg "$OUT/gpu-idle.txt")% (samples: $(tr '\n' ' ' <"$OUT/gpu-idle.txt"))"
say ""
say "| run | CPU % (top) | GPU busy % (Sphere only) | GPU busy % (whole Mac) | memory | GPU memory | Sphere's own count: CPU per frame by thread, power, wake-ups |"
say "| --- | --- | --- | --- | --- | --- | --- |"
vsync_run home "$BIN" --demo
if [ -z "${QUICK:-}" ]; then
    vsync_run home-2 "$BIN" --demo
    # what-ifs: how many threads run the app's systems, and rendering on the main thread instead of its own
    vsync_run threads-0 SPHERE_THREADS=0 "$BIN" --demo
    vsync_run threads-1 SPHERE_THREADS=1 "$BIN" --demo
    vsync_run threads-2 SPHERE_THREADS=2 "$BIN" --demo
    vsync_run threads-4 SPHERE_THREADS=4 "$BIN" --demo
    vsync_run unpipelined SPHERE_PIPELINED=0 "$BIN" --demo
    vsync_run unpipelined-threads-0 SPHERE_PIPELINED=0 SPHERE_THREADS=0 "$BIN" --demo
    vsync_run swarm "$BIN" --demo --swarm
    vsync_run fullscreen SPHERE_FS=1 "$BIN" --demo
fi
say ""

# 2. frame cost: vsync off, so frames run as fast as the slowest of main thread, render thread and GPU
bench() { # name, then env/args
    local name=$1
    shift
    quiet_machine
    env "$@" >"$OUT/bench-$name.out" 2>"$OUT/bench-$name.err"
    local line usage
    line=$(grep '^bench:' "$OUT/bench-$name.out" | tail -1)
    usage=$(grep '^usage:' "$OUT/bench-$name.out" | tail -1)
    say "| $name | ${line#bench: } | ${usage#usage: } |"
}
say "## Frame cost (vsync off, \`--bench 8\`, demo data)"
say ""
say "| view | frames | Sphere's own count |"
say "| --- | --- | --- |"
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
    bench offscreen-3200x2000 "$BIN" --bench 8 --offscreen --size 3200x2000
    bench threads-0 SPHERE_THREADS=0 "$BIN" --bench 8
    bench threads-2 SPHERE_THREADS=2 "$BIN" --bench 8
    bench unpipelined SPHERE_PIPELINED=0 "$BIN" --bench 8
fi
say ""

if [ -z "${QUICK:-}" ]; then
    # where the time goes at full speed
    quiet_machine
    "$BIN" --bench 16 >"$OUT/bench-profile.log" 2>&1 &
    pid=$!
    sleep 6
    sample "$pid" 4 -mayDie -file "$OUT/sample-bench.txt" >/dev/null 2>&1
    wait "$pid" 2>/dev/null
fi

# 3. startup: three warm launches, SPHERE_STATS=1
say "## Startup (warm, SPHERE_STATS=1)"
say ""
for i in 1 2 3; do
    quiet_machine
    SPHERE_STATS=1 "$BIN" --demo >"$OUT/startup-$i.log" 2>&1 &
    pid=$!
    sleep 7
    stop "$pid"
    say "- run $i: $(grep -E 'first frame|third frame|boot animation done' "$OUT/startup-$i.log" | sed 's/stats: //' | tr '\n' ';' | sed 's/;$//')"
done
say "- $(grep -h 'stats: entities' "$OUT"/startup-*.log | head -1 | sed 's/stats: //')"
say ""

# 4. the live System figures: Sphere runs these every 2 s (docker every 6 s) whenever it is open, outside --demo
cost() { # label, every-n-seconds, command...: CPU ms per call (user + system, children included), over 10 calls
    local what=$1 every=$2
    shift 2
    local ms
    ms=$({ /usr/bin/time -p bash -c 'for i in 1 2 3 4 5 6 7 8 9 10; do "$@" >/dev/null 2>&1; done' _ "$@"; } 2>&1 | awk '/^(user|sys) / { s += $2 } END { printf "%.1f", s * 100 }')
    say "| $what | $ms | every ${every} s | $(awk -v m="$ms" -v e="$every" 'BEGIN { printf "%.2f", m / e / 10 }') |"
}
say "## What the live System figures cost (outside --demo)"
say ""
say "| command | CPU ms per call | how often | % of one core |"
say "| --- | --- | --- | --- |"
cost "ps -A -o %cpu= (CPU)" 2 ps -A -o %cpu=
cost "vm_stat (memory)" 2 vm_stat
cost "ioreg IOAccelerator (GPU)" 2 ioreg -r -d 1 -w 0 -c IOAccelerator
cost "netstat -inb (network)" 2 netstat -inb
dk=$(for p in /usr/local/bin/docker /opt/homebrew/bin/docker /Applications/Docker.app/Contents/Resources/bin/docker; do [ -x "$p" ] && echo "$p" && break; done)
[ -n "$dk" ] && cost "docker ps -a (containers; the engine's own work not counted)" 6 "$dk" ps -a --format '{{.State}}'
say ""

# thread totals from the profiles: samples per thread, busiest first, and where they stop
for f in "$OUT"/sample-*.txt; do
    [ -f "$f" ] || continue
    say "## $(basename "$f" .txt): top-of-stack functions"
    say ""
    say '```'
    awk '/^Sort by top of stack/ { on = 1; next } on && NF { print }' "$f" | head -30 | cut -c1-140 | tee -a "$S"
    say '```'
    say ""
done
[ -f "$OUT/load-notes.txt" ] && { say "## What else was busy before each run"; say ""; cat "$OUT/load-notes.txt" | tee -a "$S"; say ""; }
echo "wrote $S"
