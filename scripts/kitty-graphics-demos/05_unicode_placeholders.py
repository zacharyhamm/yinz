#!/usr/bin/env python3
"""Display an image the way a client with no cursor addressing has to: a virtual placement
(U=1) plus a grid of U+10EEEE placeholder cells, each cell's row/column carried by combining
diacritics and its image id by the cell's foreground colour.

See `zellij-terminal-graphics/src/kitty_graphics/placeholders.rs` for the encoding this mirrors.
"""
import sys
from kittygfx import apc, emit, make_png, gradient_h

# first entries of kitty's rowcolumn-diacritics.txt -- index == encoded value
DIACRITICS = [
    "̅", "̍", "̎", "̐", "̒", "̽", "̾", "̿",
]

PLACEHOLDER = "\U0010EEEE"

IMAGE_ID = 42
COLS, ROWS = 6, 3

png = make_png(60, 30, gradient_h(60))
emit(apc(f"a=T,f=100,i={IMAGE_ID},U=1,q=2", png))

r, g, b = (IMAGE_ID >> 16) & 0xff, (IMAGE_ID >> 8) & 0xff, IMAGE_ID & 0xff
sys.stdout.write(f"\x1b[38;2;{r};{g};{b}m")
for row in range(ROWS):
    for col in range(COLS):
        sys.stdout.write(PLACEHOLDER + DIACRITICS[row] + DIACRITICS[col])
    sys.stdout.write("\x1b[0m\n")
    if row + 1 < ROWS:
        sys.stdout.write(f"\x1b[38;2;{r};{g};{b}m")
sys.stdout.flush()
