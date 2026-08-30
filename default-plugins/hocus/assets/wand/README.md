# The wand animation

The twelve frames hocus loops on an empty chat, drawn with the kitty graphics protocol.

`gen_frames.py` is the source of truth: stdlib only, deterministic, and it measures its own
canvas from the drawing, so tuning the sweep or the trail cannot start clipping a sparkle. It
writes SVG; the PNGs are what the plugin embeds, because the server already decodes PNG (`f=100`)
and shipping a GIF would mean a GIF decoder for one asset.

To regenerate after editing the script:

```sh
python3 gen_frames.py
for f in frame-*.svg; do magick -background none "$f" -depth 8 "PNG32:${f%.svg}.png"; done
magick -delay 10 -loop 0 -dispose background frame-*.png -layers optimize wand.gif
```

`magick` needs its `rsvg-convert` delegate (`magick -list delegate | grep svg`).

`gen_frames.py` prints the canvas aspect and the columns-per-row that shows it undistorted. If
that number moves, `WAND_COLS`/`WAND_ROWS` in `default-plugins/hocus/src/wand.rs` have to move
with it.

`wand.gif` is for documentation only - nothing loads it at runtime.

The SVGs are intermediates and are not committed; the PNGs and the GIF are.
