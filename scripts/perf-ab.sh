#!/usr/bin/env bash
# Compares two builds of Sphere on this Mac under the same conditions: every measurement runs for A, then for B,
# alternating for a few rounds, so a busy moment on the machine hits both. Plug the Mac in, keep the screen on and
# unlocked, and leave the Sphere windows uncovered (a covered window drops to 2 fps: such a run is retried once, then
# marked "covered"). Each run opens a Sphere window for about 20 s.
# usage: A=target/perf/bin/before/sphere-app scripts/perf-ab.sh [label] [rounds]   (B defaults to target/release/sphere-app)
#   -> target/perf/ab-<label>-<time>/summary.md, raw logs next to it
# macOS only. It never touches a Sphere that was already running, only notes it.
set -uo pipefail
cd "$(dirname "$0")/.."
A=${A:?set A to the build to compare against, e.g. A=target/perf/bin/before/sphere-app}
B=${B:-target/release/sphere-app}
label=${1:-ab}
rounds=${2:-3}
OUT=${OUT:-target/perf/ab-$label-$(date +%H%M%S)}
mkdir -p "$OUT"
S="$OUT/summary.md"
for b in "$A" "$B"; do [ -x "$b" ] || { echo "no $b"; exit 1; }; done
say() { echo "$*" | tee -a "$S"; }

. scripts/perf-lib.sh

# the display stays awake while this runs (a sleeping display covers every window)
caffeinate -dimsu -w $$ &

field() { # usage-line regex -> first number in the match
    grep -o -E "$2" <<<"$1" | head -1 | grep -o -E '[0-9]+(\.[0-9]+)?' | head -1
}
row() { # run build usage-line gpu% footprint-file
    local u=$3
    say "| $1 | $2 | $(fps_of "$u") | $(field "$u" 'CPU [0-9.]+ cores') | $(field "$u" '[0-9.]+ ms/frame') | $(field "$u" 'CPU power [0-9.]+ W') | $(field "$u" '[0-9]+ wake-ups/s') | $4 | $(fp "$5") | $(fp_gfx "$5") |"
    echo "$1	$2	$(fps_of "$u")	$(field "$u" 'CPU [0-9.]+ cores')	$(field "$u" '[0-9.]+ ms/frame')	$(field "$u" 'CPU power [0-9.]+ W')	$(field "$u" '[0-9]+ wake-ups/s')	$4	$(fp "$5" | cut -d' ' -f1)	$(fp_gfx "$5" | cut -d' ' -f1)" >>"$OUT/rows.tsv"
}

# a visible run with vsync on: Sphere counts its own CPU from 8 s to 18 s (SPHERE_STATS=1), the GPU driver its GPU time
visible() { # run build bin args...
    local run=$1 build=$2 bin=$3
    shift 3
    local try log
    for try in 1 2; do
        quiet_machine
        log="$OUT/$run-$build-$round-$try.log"
        SPHERE_STATS=1 "$bin" "$@" >"$log" 2>&1 &
        local pid=$!
        sleep 8.3
        local g0 g1
        g0=$(gpu_ns "$pid")
        sleep 9
        g1=$(gpu_ns "$pid")
        sleep 1.5
        footprint "$pid" >"$OUT/footprint-$run-$build-$round.txt" 2>&1
        stop "$pid"
        local u
        u=$(usage_of "$log")
        local f
        f=$(fps_of "$u")
        if [ -n "$f" ] && awk -v f="$f" 'BEGIN { exit !(f >= 50) }'; then
            row "$run" "$build" "$u" "$(gpu_pct "$g0" "$g1" 9)" "$OUT/footprint-$run-$build-$round.txt"
            return
        fi
        [ "$try" = 1 ] && { caffeinate -u -t 2; sleep 2; }
    done
    say "| $run | $build | covered (${f:-no usage line}) | | | | | | | |"
}

# vsync off: frames run as fast as the slowest of main thread, render thread and GPU
bench_ab() { # run build bin args...
    local run=$1 build=$2 bin=$3
    shift 3
    quiet_machine
    "$bin" "$@" >"$OUT/bench-$run-$build-$round.out" 2>"$OUT/bench-$run-$build-$round.err"
    local u
    u=$(usage_of "$OUT/bench-$run-$build-$round.out")
    say "| $run | $build | $(fps_of "$u") | $(field "$u" 'CPU [0-9.]+ cores') | $(field "$u" '[0-9.]+ ms/frame') | $(field "$u" 'CPU power [0-9.]+ W') | $(field "$u" '[0-9]+ wake-ups/s') | | | |"
    echo "$run	$build	$(fps_of "$u")	$(field "$u" 'CPU [0-9.]+ cores')	$(field "$u" '[0-9.]+ ms/frame')	$(field "$u" 'CPU power [0-9.]+ W')	$(field "$u" '[0-9]+ wake-ups/s')			" >>"$OUT/rows.tsv"
}

say "# Sphere A/B: $label"
say ""
say "- A: $A ($(du -h "$A" | cut -f1)), B: $B ($(du -h "$B" | cut -f1)); branch $(git rev-parse --short HEAD)"
say "- $(date '+%Y-%m-%d %H:%M'), $(sysctl -n machdep.cpu.brand_string), macOS $(sw_vers -productVersion), power: $(pmset -g batt | sed -n 's/.*from //p' | head -1 | tr -d "'")"
pre=$(others)
[ -n "$pre" ] && say "- a Sphere was already running (pid $(echo $pre | tr '\n' ' ')), sharing the CPU and GPU with every run"
say "- busiest processes at the start: $(load_note)"
say ""
say "| run | build | fps | CPU cores | CPU ms/frame | CPU power W | wake-ups/s | GPU busy % (Sphere) | memory | GPU memory |"
say "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
for round in $(seq "$rounds"); do
    for build in A B; do
        bin=$A
        [ "$build" = B ] && bin=$B
        visible home "$build" "$bin" --demo
        visible swarm "$build" "$bin" --demo --swarm
        bench_ab bench-home "$build" "$bin" --bench 8
        bench_ab bench-swarm "$build" "$bin" --bench 8 --swarm
    done
done
say ""
say "## Medians"
say ""
say "| run | build | fps | CPU cores | CPU ms/frame | CPU power W | wake-ups/s | GPU busy % (Sphere) | memory MB/GB | GPU memory MB |"
say "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
sort -t'	' -k1,1 -k2,2 "$OUT/rows.tsv" | awk -F'\t' '
    function med(col,   n, i, j, t, v) { n = 0; for (i = 1; i <= cnt; i++) if (val[i, col] != "" && val[i, col] != "-") v[++n] = val[i, col] + 0
        for (i = 1; i <= n; i++) for (j = i + 1; j <= n; j++) if (v[j] < v[i]) { t = v[i]; v[i] = v[j]; v[j] = t }
        if (!n) return ""; return n % 2 ? v[(n + 1) / 2] : (v[n / 2] + v[n / 2 + 1]) / 2 }
    function flush(   c, line) { if (key == "") return; line = "| " k1 " | " k2; for (c = 3; c <= 10; c++) line = line " | " med(c); print line " |"; cnt = 0 }
    { if ($1 "\t" $2 != key) { flush(); key = $1 "\t" $2; k1 = $1; k2 = $2 } cnt++; for (c = 3; c <= 10; c++) val[cnt, c] = $c }
    END { flush() }' | tee -a "$S"
say ""
echo "wrote $S"
