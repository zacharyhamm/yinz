"""Pane fixture for negotiated native RGBA delivery."""
import base64
import fcntl
import os
from pathlib import Path
import struct
import sys
import termios
import time
import zlib

root = Path(__file__).parent
deadline = time.monotonic() + 120

def wait_for(predicate):
    while not predicate():
        if time.monotonic() >= deadline:
            raise TimeoutError("native image fixture gate")
        time.sleep(0.01)

def sized():
    rows, cols, width, height = struct.unpack(
        "HHHH", fcntl.ioctl(1, termios.TIOCGWINSZ, bytes(8)))
    return rows > 0 and cols > 0 and width == cols * 8 and height == rows * 16

wait_for(sized)
print("PIXELS-READY", flush=True)
for n in range(1, int(sys.argv[1]) + 1):
    wait_for(lambda: (root / f"image-{n}").exists())
    width, height = (256, 256) if n < 33 else (4096, 4097)
    rgba = bytes((n, 72, 13, 255)) * (width * height)
    payload = base64.b64encode(zlib.compress(rgba, 1))
    os.write(1, f"\033[3;{n}H".encode())
    for start in range(0, len(payload), 4096):
        more = int(start + 4096 < len(payload))
        params = f"a=T,f=32,s={width},v={height},i={n},C=1,o=z," if start == 0 else ""
        sys.stdout.buffer.write(f"\033_G{params}q=2,m={more};".encode() + payload[start:start+4096] + b"\033\\")
    sys.stdout.buffer.flush()
for gate in ("during", "after"):
    wait_for(lambda: (root / gate).exists())
    print("\033[1;1H" + gate.upper() + "-DRAIN", flush=True)
wait_for(lambda: False)
