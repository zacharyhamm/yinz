#!/usr/bin/env bash
#
# Run every demo in this directory in order, pausing between each so you can look at the
# result before the next one clears/overwrites it. Run this *inside* a zellij pane (or any
# other kitty-protocol host) — it writes raw escape sequences to stdout.
#
#   scripts/kitty-graphics-demos/run_all.sh
#   scripts/kitty-graphics-demos/run_all.sh 04_placement_scaling.py   # just one

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

demos=(01_basic_png.py 02_raw_rgb_rgba.py 03_chunked_transfer.py 04_placement_scaling.py \
       05_unicode_placeholders.py 06_animation.py 07_relative_placement.py 08_deletion.py)

targets=("${@:-${demos[@]}}")

for demo in "${targets[@]}"; do
    clear
    echo "=== $demo ==="
    python3 "$demo"
    echo
    read -r -p "-- press enter for the next demo (ctrl-c to stop) --" _
done
