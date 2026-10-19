# Helpers shared by perf.sh and perf-ab.sh (sourced, not run). macOS only.

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
# fps, CPU cores and the rest of Sphere's own usage line in a log ("stats: usage, ..." or "usage: ...")
usage_of() { grep -h -E '^(stats: )?usage' "$1" | tail -1 | sed -E 's/^(stats: )?usage:? ?,? ?//'; }
fps_of() { grep -o '([0-9.]* fps)' <<<"$1" | head -1 | tr -d '(' | cut -d' ' -f1; }

