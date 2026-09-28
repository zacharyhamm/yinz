#!/usr/bin/env bash
# Fire synthetic Claude Code hook events at a pane, the way a busy agent does.
#
#   scripts/hook-storm.sh <pane_id> [rate_per_second] [seconds]
#
# Run it from inside the session under test (it uses `yinz action`, which
# resolves the session from $YINZ_SESSION_NAME), pointing at any *terminal*
# pane id — a shell sitting at a prompt is fine. It alternates PreToolUse /
# PostToolUse with a changing tool name, so every event flips the pane's agent
# record exactly like a real working agent and the whole report → fan-out path
# runs each time. Pair it with scripts/plugin-thread-cpu.sh and the
# `plugin-exec-N job stats` lines in the yinz log; the methodology lives in
# docs/PLUGIN_RENDER_PIPELINE.md.
set -euo pipefail

# Which yinz to drive — a lab run points this at a checkout build.
yinz=${YINZ_BIN:-yinz}

pane_id=${1:?usage: hook-storm.sh <pane_id> [rate_per_second] [seconds]}
rate=${2:-3}
seconds=${3:-30}

total=$((rate * seconds))
interval=$(awk -v r="$rate" 'BEGIN { printf "%.4f", 1 / r }')
session_id="hook-storm-$$"

echo "hook-storm: $total events at ${rate}/s against pane $pane_id" >&2
i=0
start=$(date +%s.%N)
while [ "$i" -lt "$total" ]; do
    if [ $((i % 2)) -eq 0 ]; then
        event=PreToolUse
    else
        event=PostToolUse
    fi
    printf '{"hook_event_name":"%s","session_id":"%s","tool_name":"Bash","tool_input":{"command":"synthetic hook-storm event %d"}}' \
        "$event" "$session_id" "$i" \
        | "$yinz" action agent-report --hook-json --pane-id "$pane_id" >/dev/null 2>&1 || true
    i=$((i + 1))
    # Pace against the wall clock rather than sleeping the interval, so slow
    # `yinz action` calls do not stretch the run silently.
    due=$(awk -v s="$start" -v i="$i" -v d="$interval" 'BEGIN { printf "%.4f", s + i * d }')
    now=$(date +%s.%N)
    pause=$(awk -v d="$due" -v n="$now" 'BEGIN { p = d - n; if (p < 0) p = 0; printf "%.4f", p }')
    sleep "$pause"
done
elapsed=$(awk -v s="$start" -v e="$(date +%s.%N)" 'BEGIN { printf "%.1f", e - s }')
echo "hook-storm: sent $total events in ${elapsed}s" >&2
# Leave the pane as we found it: no agent record.
"$yinz" action agent-report --pane-id "$pane_id" --clear >/dev/null 2>&1 || true
