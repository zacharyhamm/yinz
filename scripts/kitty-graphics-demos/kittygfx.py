"""Shared helpers for the kitty graphics protocol demo scripts in this directory.

Stdlib only (no Pillow) — PNGs are hand-built with zlib, since that's all a demo image needs.
Run these against a real terminal (inside zellij, or against any kitty-protocol host) to see
escape sequences reach `zellij-terminal/src/panes/kitty_graphics`. They print nothing but the
raw protocol on stdout, so redirecting to a file gives you replayable fixtures too.
"""
import base64
import struct
import sys
import zlib

APC_CHUNK_LIMIT = 4096  # matches the 4096-byte-payload chunking kitty clients commonly use


def apc(control, payload=b""):
    """ESC _G <control> [ ; base64(payload) ] ESC \\"""
    out = b"\x1b_G" + control.encode("ascii")
    if payload:
        out += b";" + base64.b64encode(payload)
    out += b"\x1b\\"
    return out


def emit(*chunks):
    for c in chunks:
        sys.stdout.buffer.write(c)
    sys.stdout.buffer.flush()


def emit_chunked(control_first, payload, chunk_size=APC_CHUNK_LIMIT):
    """Split payload across multiple APC commands using m=1/m=0, as real clients do for big images."""
    b64 = base64.b64encode(payload)
    chunks = [b64[i:i + chunk_size] for i in range(0, len(b64), chunk_size)] or [b""]
    out = []
    for i, chunk in enumerate(chunks):
        last = i == len(chunks) - 1
        if i == 0:
            control = control_first + (",m=0" if last else ",m=1")
        else:
            control = "m=" + ("0" if last else "1")
        control_bytes = b"\x1b_G" + control.encode("ascii") + b";" + chunk + b"\x1b\\"
        out.append(control_bytes)
    return b"".join(out)


def make_png(width, height, pixel_fn):
    """A minimal RGBA PNG, hand-rolled with zlib so the demos need no image library.

    pixel_fn(x, y) -> (r, g, b, a)
    """
    def chunk(tag, data):
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xffffffff)

    raw = bytearray()
    for y in range(height):
        raw.append(0)  # filter: none
        for x in range(width):
            raw.extend(pixel_fn(x, y))
    compressed = zlib.compress(bytes(raw), 9)

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)  # 8-bit RGBA, no interlace
    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", compressed) + chunk(b"IEND", b"")


def solid_rgba(r, g, b, a=255):
    return lambda x, y: (r, g, b, a)


def checkerboard(size=4, c1=(255, 0, 0, 255), c2=(0, 0, 255, 255)):
    return lambda x, y: c1 if ((x // size) + (y // size)) % 2 == 0 else c2


def gradient_h(w):
    return lambda x, y: (int(255 * x / max(w - 1, 1)), 80, int(255 * (1 - x / max(w - 1, 1))), 255)


def raw_rgb(width, height, pixel_fn):
    """3-byte-per-pixel raw data for f=24."""
    out = bytearray()
    for y in range(height):
        for x in range(width):
            r, g, b, _a = pixel_fn(x, y)
            out.extend((r, g, b))
    return bytes(out)


def raw_rgba(width, height, pixel_fn):
    """4-byte-per-pixel raw data for f=32."""
    out = bytearray()
    for y in range(height):
        for x in range(width):
            out.extend(pixel_fn(x, y))
    return bytes(out)
