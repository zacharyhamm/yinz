# kitty graphics protocol demos

Small standalone Python scripts (stdlib only — no Pillow) that emit real kitty graphics
protocol escape sequences, for manually exercising `zellij-server/src/panes/kitty_graphics`
in a running terminal. These complement the automated conformance suite
(`zellij-server/src/panes/kitty_graphics`, `zellij-integration-tests/tests/kitty_graphics*`);
they're for looking at the result with your own eyes, e.g. inside a zellij pane, or for
comparing zellij's rendering against a real kitty/wezterm/ghostty.

## Run

```sh
cargo x build && cargo x run   # or run these against any other kitty-protocol terminal
python3 scripts/kitty-graphics-demos/01_basic_png.py
# ...or step through all of them:
scripts/kitty-graphics-demos/run_all.sh
```

Each script writes raw APC sequences to stdout — run it directly in a terminal, don't pipe
its output anywhere except a real terminal device (`09_query.py` also reads the reply back
from stdin, so it needs a tty on both ends).

The display demos use `q=2` because they do not read protocol replies. Without it, a terminal's
`OK` response can be echoed by the cooked tty as visible `^[_G...` text. The query demo instead
puts stdin in raw mode before sending its request, reads the reply, and restores the tty.

## What each one exercises

| script | protocol surface |
|---|---|
| `01_basic_png.py` | `a=T,f=100` — transmit + display a PNG in one command |
| `02_raw_rgb_rgba.py` | `f=24`/`f=32` raw pixel data, transmit (`a=t`) and display (`a=p`) as separate steps |
| `03_chunked_transfer.py` | multi-chunk transmission via `m=1`/`m=0` |
| `04_placement_scaling.py` | multiple placements of one image: natural size, `c=`/`r=` cell scaling, `X`/`Y` sub-cell offset, `z` stacking |
| `05_unicode_placeholders.py` | virtual placement (`U=1`) + `U+10EEEE` placeholder grid with row/column diacritics and id-bearing foreground colour — see `kitty_graphics/placeholders.rs` |
| `06_animation.py` | base frame + `a=f` additional frames + `a=a,c=<n>` frame selection (no autoplay clock — see the module doc on `KittyAnimation`) |
| `07_relative_placement.py` | a placement anchored to another image's placement via `P=`/`Q=` with `H=`/`V=` offset |
| `08_deletion.py` | delete variants: `d=i` (placement only) vs `d=I` (placement + free image data) vs `d=A` (delete all visible) |
| `09_query.py` | `a=q` capability query and reading the terminal's reply |

`kittygfx.py` is the shared helper module (APC framing, chunking, a from-scratch PNG encoder
built on stdlib `zlib`/`struct`). Import it, don't copy it, if you add another demo.
