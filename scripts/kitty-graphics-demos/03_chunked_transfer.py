#!/usr/bin/env python3
"""A PNG large enough to need multiple APC chunks (m=1 ... m=0), like a real client sends."""
from kittygfx import emit, emit_chunked, make_png, gradient_h

png = make_png(256, 256, gradient_h(256))
emit(emit_chunked("a=T,f=100,i=20", png, chunk_size=4096))
print()
