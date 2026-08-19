#!/usr/bin/env python3
"""A virtual parent placement, and a second image placed relative to it (P=<parent image>,
Q=<parent placement>) with a cell offset -- the case in
`grid_tests::kitty_relative_placement`."""
from kittygfx import apc, emit, make_png, solid_rgba, checkerboard

PARENT_ID, CHILD_ID = 60, 61

parent = make_png(20, 20, checkerboard(size=4, c1=(200, 200, 200, 255), c2=(60, 60, 60, 255)))
emit(apc(f"a=T,f=100,i={PARENT_ID},q=2", parent))
emit(apc(f"a=p,i={PARENT_ID},p=1"))

child = make_png(12, 12, solid_rgba(255, 140, 0))
emit(apc(f"a=T,f=100,i={CHILD_ID},q=2", child))
# placed 3 columns / 1 row offset from the parent's placement 1
emit(apc(f"a=p,i={CHILD_ID},P={PARENT_ID},Q=1,H=3,V=1"))
print()
