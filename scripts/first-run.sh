#!/usr/bin/env bash
# Run this checkout with disposable first-run state. Quit or detach to clean up.
set -euo pipefail

if (( $# != 0 )); then
    echo "usage: $0" >&2
    exit 2
fi

repo=$(cd -- "$(dirname -- "$0")/.." && pwd)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/yinz-first-run.XXXXXX")
session="first-run-${scratch##*.}-$$"

cleanup() {
    # Always name our own session: never fall back to the invoking session.
    # A detached client can leave this server running after cargo returns.
    if [[ -x "$repo/target/debug/yinz" ]]; then
        "$repo/target/debug/yinz" --data-dir "$scratch/data" \
            kill-session "$session" >/dev/null 2>&1 || true
    fi
    rm -rf -- "$scratch"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir -p "$scratch/config"
printf 'First-run session: %s\nTemporary files: %s\n' "$session" "$scratch"
cd -- "$repo"
# cargo x run installs the builtin plugins into the selected data directory.
# An explicit --config requires an existing file and skips the missing-config
# startup path. Clear the inherited override so Yinz can create its own config.
unset YINZ_CONFIG_FILE
cargo x run --data-dir "$scratch/data" -- \
    --config-dir "$scratch/config" \
    --session "$session"
