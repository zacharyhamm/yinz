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

def transmit(n, width, height, params=""):
    rgba = bytes((n % 256, 72, 13, 255)) * (width * height)
    payload = base64.b64encode(zlib.compress(rgba, 1))
    for start in range(0, len(payload), 4096):
        more = int(start + 4096 < len(payload))
        head = f"a=T,f=32,s={width},v={height},{params}C=1,o=z," if start == 0 else ""
        sys.stdout.buffer.write(f"\033_G{head}q=2,m={more};".encode() + payload[start:start+4096] + b"\033\\")
    sys.stdout.buffer.flush()

wait_for(sized)
print("PIXELS-READY", flush=True)
if sys.argv[1] == "replace":
    # Each replace-N retransmits image 1 as a new 16 MiB raster, shown through a small
    # source rectangle, then names the replacement in text for liveness waits.
    n = 0
    while True:
        n += 1
        wait_for(lambda: (root / f"replace-{n}").exists())
        os.write(1, b"\033[5;1H")
        transmit(n, 2048, 2048, "i=1,x=0,y=0,w=64,h=64,")
        print(f"\033[1;1HREPLACED-{n:03}", flush=True)
if sys.argv[1] == "large":
    # One 5120x5120 RGBA image, exactly the 100 MiB decode limit, then a text marker
    # that must arrive in order with the chunked frames around it.
    wait_for(lambda: (root / "image-1").exists())
    os.write(1, b"\033[3;1H")
    transmit(1, 5120, 5120, "i=1,")
    print("\033[1;1HLARGE-SENT", flush=True)
    wait_for(lambda: False)
if sys.argv[1] == "ordered":
    # A threshold-sized image, then two 16 MiB images shown through small source
    # rectangles, which span frames and so wait for the chunk window. Each is followed
    # by a marker naming it.
    for n in range(1, 4):
        wait_for(lambda: (root / f"image-{n}").exists())
        os.write(1, f"\033[3;{8 * n}H".encode())
        if n == 1:
            transmit(n, 256, 256, f"i={n},")
        else:
            transmit(n, 2048, 2048, f"i={n},x=0,y=0,w=64,h=64,")
        print(f"\033[1;1HSENT-{n:03}", flush=True)
    wait_for(lambda: False)
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
