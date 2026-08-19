#!/usr/bin/env python3
"""Delete variants: by placement id (d=i, uppercase frees the image), by image id (d=I), and
'everything visible' (d=a / d=A). Run interactively -- each block pauses so you can see the
image before and after its delete fires."""
import time
from kittygfx import apc, emit, make_png, solid_rgba

def show(msg):
    print(f"\n--- {msg} ---")


img_a = make_png(16, 16, solid_rgba(220, 30, 30))
img_b = make_png(16, 16, solid_rgba(30, 120, 220))

show("placing two images (ids 70, 71)")
emit(apc("a=T,f=100,i=70,q=2", img_a))
emit(apc("a=p,i=70,p=1"))
print()
emit(apc("a=T,f=100,i=71,q=2", img_b))
emit(apc("a=p,i=71,p=1"))
print()
time.sleep(1.5)

show("d=i,i=70,p=1 (delete placement only, image data kept)")
emit(apc("a=d,d=i,i=70,p=1"))
time.sleep(1.0)
show("re-placing 70 to show its data survived")
emit(apc("a=p,i=70,p=2"))
print()
time.sleep(1.5)

show("d=I,i=70 (uppercase: delete placements AND free image data)")
emit(apc("a=d,d=I,i=70"))
time.sleep(1.0)

show("d=A (delete every visible placement)")
emit(apc("a=d,d=A"))
print()
