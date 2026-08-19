#!/usr/bin/env python3
"""Transmit + display a small PNG in one command (a=T, f=100, direct payload)."""
from kittygfx import apc, emit, make_png, checkerboard

png = make_png(32, 32, checkerboard(size=4))
emit(apc("a=T,f=100,i=1,q=2", png))
print()
