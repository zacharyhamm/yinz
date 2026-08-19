#!/usr/bin/env python3
"""Transmit a base frame, add two more frames (a=f), then switch the displayed frame with
a=a,c=<n>. Per the module docs, zellij composes and selects real frames but has no playback
clock, so a=a,s=2 (autoplay) is accepted and ignored -- driving c= yourself is what actually
changes the screen."""
import time
from kittygfx import apc, emit, make_png, solid_rgba

IMAGE_ID = 50
W, H = 24, 24

frame_colors = [(220, 40, 40), (40, 200, 80), (60, 90, 230)]

base = make_png(W, H, solid_rgba(*frame_colors[0]))
emit(apc(f"a=T,f=100,i={IMAGE_ID},q=2", base))
emit(apc(f"a=p,i={IMAGE_ID},p=1"))
print()

for color in frame_colors[1:]:
    frame_png = make_png(W, H, solid_rgba(*color))
    emit(apc(f"a=f,i={IMAGE_ID},f=100,q=2", frame_png))

for n in range(1, len(frame_colors) + 1):
    emit(apc(f"a=a,i={IMAGE_ID},c={n}"))
    time.sleep(0.6)
