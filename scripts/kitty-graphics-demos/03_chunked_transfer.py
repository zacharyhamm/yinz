#!/usr/bin/env python3
"""A PNG large enough to need multiple APC chunks (m=1 ... m=0), like a real client sends."""
import random
from kittygfx import emit, emit_chunked, make_png

rng = random.Random(0)
png = make_png(256, 256, lambda _x, _y: (rng.randrange(256), rng.randrange(256), rng.randrange(256), 255))
emit(emit_chunked("a=T,f=100,i=20,q=2", png, chunk_size=4096))
print()
