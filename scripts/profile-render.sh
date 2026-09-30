#!/usr/bin/env bash
# Capture both sides of a live Yinz render pipeline. See docs/render-profiling.md.
set -euo pipefail

if (( $# < 3 || $# > 4 )); then
    echo "usage: $0 <server-pid> <client-pid> <new-output-directory> [seconds=30]" >&2
    exit 2
fi
server_pid=$1
client_pid=$2
output=$3
seconds=${4:-30}
for value in "$server_pid" "$client_pid" "$seconds"; do
    if [[ ! $value =~ ^[1-9][0-9]*$ ]]; then
        echo "PIDs and seconds must be positive integers" >&2
        exit 2
    fi
done
if [[ $server_pid == "$client_pid" ]]; then
    echo "Select separate server and client processes" >&2
    exit 2
fi
command -v samply >/dev/null
kill -0 "$server_pid" "$client_pid"
# Refuse an existing directory so a second run cannot overwrite the first profile.
mkdir -- "$output"
repo=$(cd -- "$(dirname -- "$0")/.." && pwd)
{
    date -u +%FT%TZ
    git -C "$repo" rev-parse HEAD
    git -C "$repo" status --short
    uname -a
    samply --version
    printf 'duration_seconds=%s\n' "$seconds"
    ps -p "$server_pid,$client_pid" -o pid=,args=
} > "$output/metadata.txt"

recorders=()
# Called by the EXIT trap, including after an interrupted recording.
# shellcheck disable=SC2317
cleanup() {
    for pid in "${recorders[@]}"; do
        kill -INT "$pid" 2>/dev/null || true
    done
    for pid in "${recorders[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for role in server client; do
    pid=$server_pid
    [[ $role != client ]] || pid=$client_pid
    samply record --pid "$pid" --duration "$seconds" --cswitch-markers \
        --save-only --output "$output/$role.json.gz" \
        > "$output/$role.log" 2>&1 &
    recorders+=("$!")
done

# Constant storage per sample and a finite recording window. CPU time is cumulative, %CPU
# is a process-lifetime average, RSS is KiB. On Linux, VmHWM is the lifetime peak RSS.
for (( sample=0; sample<=seconds; sample++ )); do
    {
        date -u +%FT%TZ
        ps -p "$server_pid,$client_pid" -o pid=,time=,pcpu=,rss= || true
        for pid in "$server_pid" "$client_pid"; do
            if [[ -r /proc/$pid/status ]]; then
                awk -v pid="$pid" '/^VmHWM:/ {print pid, $0}' "/proc/$pid/status"
            fi
        done
    } >> "$output/resources.txt"
    if ! kill -0 "${recorders[0]}" 2>/dev/null && ! kill -0 "${recorders[1]}" 2>/dev/null; then
        break
    fi
    if (( sample < seconds )); then sleep 1; fi
done

status=0
for pid in "${recorders[@]}"; do
    wait "$pid" || status=1
done
recorders=()
if (( status )); then
    echo "Profiling failed. Read $output/server.log and $output/client.log." >&2
else
    echo "Profiles and resource samples saved in $output"
fi
exit "$status"
