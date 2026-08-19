#!/usr/bin/env python3
"""Transmit raw RGB (f=24) and RGBA (f=32) pixel data, each transmitted then displayed
separately (a=t then a=p), rather than the combined a=T shortcut."""
from kittygfx import apc, emit, raw_rgb, raw_rgba, gradient_h, solid_rgba

w, h = 20, 10

rgb = raw_rgb(w, h, gradient_h(w))
emit(apc(f"a=t,f=24,s={w},v={h},i=10,q=2", rgb))
emit(apc("a=p,i=10,q=2"))
print()

rgba = raw_rgba(w, h, solid_rgba(0, 200, 120, 128))
emit(apc(f"a=t,f=32,s={w},v={h},i=11,q=2", rgba))
emit(apc("a=p,i=11,q=2"))
print()
