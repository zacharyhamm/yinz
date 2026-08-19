#!/usr/bin/env python3
"""a=q: ask whether the host supports the protocol/format without displaying anything. Reads
the reply back from stdin, so run this directly in a terminal (not piped)."""
import sys
import termios
import tty
from kittygfx import apc, emit, make_png, solid_rgba

tiny = make_png(1, 1, solid_rgba(255, 255, 255))
emit(apc("a=q,i=1,f=100", tiny))

fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
try:
    tty.setraw(fd)
    reply = b""
    while not reply.endswith(b"\x1b\\"):
        reply += sys.stdin.buffer.read(1)
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)

print("\r\nreply:", reply)
