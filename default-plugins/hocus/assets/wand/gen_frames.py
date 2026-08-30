#!/usr/bin/env python3
"""Draw the wand animation hocus shows on an empty chat, one SVG per frame.

Stdlib only, and deterministic: same input, same bytes out, so a regenerated frame set is an
empty diff unless something here actually changed. The SVGs are intermediate - see README.md for
the one `magick` line that turns them into the PNGs the plugin embeds.

The wand sweeps a short arc, a star glints at its tip on a stagger, and a trail of sparkles
follows the tip and fades. Nothing is anti-aliased by hand; rsvg does that on the way to PNG.
"""

import math
import pathlib

FRAMES = 12
MARGIN = 14.0

# The wand sits to the right and sweeps a short arc; the sparkles it throws stream away to the
# left, which is what fills a wide banner rather than leaving half of it empty.
PIVOT = (335.0, 300.0)
LENGTH = 200.0
ARC_FROM, ARC_TO = -76.0, -44.0

WAND = "#d8d2e8"
HANDLE = "#4b4260"
GLINT = "#ffe9a8"
SPARK = "#a8e6ff"


def ease(t: float) -> float:
    """Ease in and out, and land back where it started so the loop has no seam."""
    return (1.0 - math.cos(2.0 * math.pi * t)) / 2.0


def tip(frame: int) -> tuple[float, float]:
    angle = math.radians(ARC_FROM + (ARC_TO - ARC_FROM) * ease(frame / FRAMES))
    return (PIVOT[0] + LENGTH * math.cos(angle), PIVOT[1] + LENGTH * math.sin(angle))


def star(cx: float, cy: float, radius: float, opacity: float, color: str) -> str:
    """A four-pointed sparkle: two crossed tapered diamonds, which reads as a glint at any size."""
    waist = radius / 4.0
    points = " ".join(
        f"{cx + dx:.1f},{cy + dy:.1f}"
        for dx, dy in (
            (0, -radius), (waist, -waist), (radius, 0), (waist, waist),
            (0, radius), (-waist, waist), (-radius, 0), (-waist, -waist),
        )
    )
    return f'<polygon points="{points}" fill="{color}" opacity="{opacity:.2f}"/>'


def sparkles(frame: int) -> list[str]:
    """Sparkles are born at the tip and drift up and out, each on its own offset in the loop.

    A spark is born at frame `born_at` and lives exactly one loop, so its age wraps with the
    animation and the last frame hands over to the first without a seam. Its origin is the tip
    *at the frame it was born* - reading the current tip instead is what made the trail snap back
    to the wand once a loop.
    """
    out = []
    count = 11
    for index in range(count):
        born_at = round(index * FRAMES / count) % FRAMES
        age = ((frame - born_at) % FRAMES) / FRAMES
        # Fade in fast, out slowly: a spark is brightest just after it appears, and the long tail
        # is what makes the trail read as a trail rather than as a ring around the tip.
        opacity = min(1.0, age * 6.0) * (1.0 - age)
        if opacity < 0.02:
            continue
        origin_x, origin_y = tip(born_at)
        # Deterministic jitter, so the sparks scatter instead of marching along eleven spokes.
        wobble = math.sin(index * 12.9898) * 14.0
        angle = math.radians(-196.0 + 58.0 * (index / (count - 1)) + wobble)
        drift = 215.0 * age
        x = origin_x + drift * math.cos(angle)
        y = origin_y + drift * math.sin(angle)
        out.append(star(x, y, 4.0 + 8.0 * (1.0 - age), opacity, SPARK))
    return out


def frame_parts(frame: int) -> list[str]:
    tip_x, tip_y = tip(frame)
    # The wand tapers, so it is a quad rather than a stroked line.
    across = math.radians(math.degrees(math.atan2(tip_y - PIVOT[1], tip_x - PIVOT[0])) + 90.0)
    ax, ay = math.cos(across), math.sin(across)
    body = (
        f'<polygon points="'
        f'{PIVOT[0] + 9 * ax:.1f},{PIVOT[1] + 9 * ay:.1f} '
        f'{PIVOT[0] - 9 * ax:.1f},{PIVOT[1] - 9 * ay:.1f} '
        f'{tip_x - 3 * ax:.1f},{tip_y - 3 * ay:.1f} '
        f'{tip_x + 3 * ax:.1f},{tip_y + 3 * ay:.1f}" fill="{WAND}"/>'
    )
    handle = (
        f'<polygon points="'
        f'{PIVOT[0] + 10 * ax:.1f},{PIVOT[1] + 10 * ay:.1f} '
        f'{PIVOT[0] - 10 * ax:.1f},{PIVOT[1] - 10 * ay:.1f} '
        f'{PIVOT[0] - 8 * ax - 0.32 * (tip_x - PIVOT[0]):.1f},'
        f'{PIVOT[1] - 8 * ay - 0.32 * (tip_y - PIVOT[1]):.1f} '
        f'{PIVOT[0] + 8 * ax - 0.32 * (tip_x - PIVOT[0]):.1f},'
        f'{PIVOT[1] + 8 * ay - 0.32 * (tip_y - PIVOT[1]):.1f}" fill="{HANDLE}"/>'
    )
    # The tip glint pulses twice per loop, a beat ahead of the sweep.
    glint_pulse = 0.55 + 0.45 * math.sin(2.0 * math.pi * (frame / FRAMES) * 2.0)
    parts = [
        handle,
        body,
        star(tip_x, tip_y, 26.0 + 10.0 * glint_pulse, 1.0, GLINT),
        star(tip_x, tip_y, 12.0, 1.0, "#fffdf5"),
        *sparkles(frame),
    ]
    return parts


def bounds() -> tuple[float, float, float, float]:
    """The box every frame's drawing fits inside, plus a margin.

    Measured rather than hardcoded, so tuning the sweep or the trail cannot silently start
    clipping a sparkle. Every frame is cropped to the same box, so nothing jitters.
    """
    xs: list[float] = []
    ys: list[float] = []
    for frame in range(FRAMES):
        for polygon in frame_parts(frame):
            points = polygon.split('points="', 1)[1].split('"', 1)[0]
            for pair in points.split():
                x, y = pair.split(",")
                xs.append(float(x))
                ys.append(float(y))
    return (
        min(xs) - MARGIN,
        min(ys) - MARGIN,
        max(xs) - min(xs) + 2 * MARGIN,
        max(ys) - min(ys) + 2 * MARGIN,
    )


def frame_svg(frame: int, view: tuple[float, float, float, float]) -> str:
    x, y, width, height = view
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" '
        f'width="{width:.0f}" height="{height:.0f}" '
        f'viewBox="{x:.1f} {y:.1f} {width:.1f} {height:.1f}">'
        + "".join(frame_parts(frame))
        + "</svg>\n"
    )


def main() -> None:
    here = pathlib.Path(__file__).parent
    view = bounds()
    for frame in range(FRAMES):
        (here / f"frame-{frame:02}.svg").write_text(frame_svg(frame, view))
    aspect = view[2] / view[3]
    print(f"wrote {FRAMES} frames to {here}")
    print(f"canvas {view[2]:.0f}x{view[3]:.0f}, aspect {aspect:.2f}")
    # A terminal cell is roughly half as wide as it is tall, so this is the cell box that shows
    # the image undistorted. The plugin's own constants have to agree with it.
    print(f"suggested placement: {aspect / 0.5:.2f} columns per row")


if __name__ == "__main__":
    main()
