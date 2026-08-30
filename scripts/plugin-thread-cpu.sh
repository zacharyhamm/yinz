#!/usr/bin/env bash
# Sample a zellij server's plugin-executor and screen threads' CPU, per second.
#
#   scripts/plugin-thread-cpu.sh <session_name> [seconds] [hook_count]
#
# Prints one line per second with the CPU-ms each plugin-exec-*/screen/wasm
# thread burned, a total per thread at the end — and, when hook_count is given
# (the number of events scripts/hook-storm.sh sent during the window), CPU-ms
# per hook per thread, which is the before/after number
# docs/PLUGIN_RENDER_PIPELINE.md tracks. Per-plugin attribution comes from the
# `plugin-exec-N job stats` lines the server logs every 10s.
set -euo pipefail

# Which zellij to drive — a lab run points this at a checkout build.
zellij=${ZELLIJ_BIN:-zellij}

session=${1:?usage: plugin-thread-cpu.sh <session_name> [seconds] [hook_count]}
seconds=${2:-30}
hooks=${3:-}

pid=$(pgrep -f "zellij --server .*/${session}$" | head -1)
[ -n "$pid" ] || { echo "no server found for session '$session'" >&2; exit 1; }
tick=$(getconf CLK_TCK)

declare -A prev total
snap() {
    for t in /proc/"$pid"/task/*; do
        c=$(cat "$t/comm" 2>/dev/null) || continue
        case "$c" in plugin-exec-*|screen|wasm) echo "$c $(awk '{print $14+$15}' "$t/stat" 2>/dev/null || echo 0)";; esac
    done
}
while read -r c v; do prev[$c]=$v; total[$c]=0; done < <(snap)

for i in $(seq 1 "$seconds"); do
    sleep 1
    line="t=$i"
    while read -r c v; do
        p=${prev[$c]:-$v}
        d=$((v - p))
        prev[$c]=$v
        total[$c]=$(( ${total[$c]:-0} + d ))
        [ "$d" -gt 0 ] && line="$line $c=+$((d * 1000 / tick))ms"
    done < <(snap)
    echo "$line"
done

echo "-- totals over ${seconds}s (pid $pid, session $session)"
for c in $(printf '%s\n' "${!total[@]}" | sort); do
    ms=$(( ${total[$c]} * 1000 / tick ))
    if [ -n "$hooks" ] && [ "$hooks" -gt 0 ]; then
        echo "$c: ${ms}ms total, $((ms / hooks))ms/hook"
    else
        echo "$c: ${ms}ms total"
    fi
done
