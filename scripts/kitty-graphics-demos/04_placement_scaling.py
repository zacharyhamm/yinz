#!/usr/bin/env python3
"""One transmitted image, several placements: natural size, c= / r= cell scaling, X/Y cell
offset, and z-index stacking order."""
from kittygfx import apc, emit, make_png, checkerboard

png = make_png(40, 40, checkerboard(size=5))
emit(apc("a=t,f=100,i=30,q=2", png))  # transmit only, suppress OK reply

# natural size placement
emit(apc("a=p,i=30,p=1,q=2"))
print()

# scaled to exactly 10 columns x 4 rows
emit(apc("a=p,i=30,p=2,c=10,r=4,q=2"))
print()

# same image, offset within its top-left cell, placed above/below via z
emit(apc("a=p,i=30,p=3,c=8,r=3,X=2,Y=2,z=5,q=2"))
emit(apc("a=p,i=30,p=4,c=8,r=3,z=-5,q=2"))
print()
