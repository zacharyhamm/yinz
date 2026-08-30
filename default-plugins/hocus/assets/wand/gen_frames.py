#!/usr/bin/env python3
"""Draw the wand animation hocus shows on an empty chat, one SVG per frame.

Stdlib only, and deterministic: same input, same bytes out, so a regenerated frame set is an
empty diff unless something here actually changed. The SVGs are intermediate - see README.md for
the one `magick` line that turns them into the PNGs the plugin embeds.

The wand sweeps a short arc through a glowing trail, orbiting sigils and a burst of varied
sparks. Nothing is anti-aliased by hand; rsvg does that on the way to PNG.
"""

import math
import pathlib

FRAMES = 12
MARGIN = 20.0
DISPLAY_COLS, DISPLAY_ROWS = 10, 6
TARGET_ASPECT = DISPLAY_COLS * 0.5 / DISPLAY_ROWS

# The wand sits to the right and sweeps a short arc; the sparkles it throws stream away to the
# left, which is what fills a wide banner rather than leaving half of it empty.
PIVOT = (335.0, 300.0)
LENGTH = 200.0
ARC_FROM, ARC_TO = -76.0, -44.0

HANDLE = "#4b4260"
GLINT = "#ffe9a8"
SPARKS = ("#a8e6ff", "#d8b4ff", "#fff0a8")


def ease(t: float) -> float:
    """Ease in and out, and land back where it started so the loop has no seam."""
    return (1.0 - math.cos(2.0 * math.pi * t)) / 2.0


def tip(frame: float) -> tuple[float, float]:
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


def trail(frame: int) -> list[str]:
    """A tapered ribbon following the last few subframes of the wand's curved sweep."""
    points = [tip(frame - step * 0.55) for step in range(7)]
    out = []
    for index, ((x1, y1), (x2, y2)) in enumerate(zip(points, points[1:])):
        dx, dy = x2 - x1, y2 - y1
        length = math.hypot(dx, dy) or 1.0
        nx, ny = -dy / length, dx / length
        near = 10.0 - index
        far = max(1.5, near - 2.5)
        polygon = " ".join(
            f"{x:.1f},{y:.1f}"
            for x, y in (
                (x1 + nx * near, y1 + ny * near),
                (x1 - nx * near, y1 - ny * near),
                (x2 - nx * far, y2 - ny * far),
                (x2 + nx * far, y2 + ny * far),
            )
        )
        color = SPARKS[index % 2]
        out.append(
            f'<polygon points="{polygon}" fill="{color}" opacity="{0.34 - index * 0.04:.2f}"/>'
        )
    return out


def sigils(frame: int, cx: float, cy: float) -> list[str]:
    """Three small arcane marks orbit the tip at different radii and speeds."""
    out = []
    for index, radius in enumerate((45.0, 58.0, 70.0)):
        phase = 2.0 * math.pi * (frame / FRAMES) * (index + 1) + index * 2.1
        x = cx + radius * math.cos(phase)
        y = cy + radius * 0.55 * math.sin(phase)
        out.append(star(x, y, 5.5 + index, 0.55 + 0.12 * index, SPARKS[index]))
    return out


def burst(frame: int, cx: float, cy: float) -> list[str]:
    """One radial pulse per loop, gone before the last frame meets the first."""
    age = frame / FRAMES
    opacity = (1.0 - age) ** 2
    radius = 24.0 + 95.0 * age
    return [
        star(
            cx + radius * math.cos(index * math.pi / 4.0),
            cy + radius * math.sin(index * math.pi / 4.0),
            3.0 + 6.0 * (1.0 - age),
            opacity,
            SPARKS[index % len(SPARKS)],
        )
        for index in range(8)
    ]


def sparkles(frame: int) -> list[str]:
    """Sparkles are born at the tip and drift up and out, each on its own offset in the loop.

    A spark is born at frame `born_at` and lives exactly one loop, so its age wraps with the
    animation and the last frame hands over to the first without a seam. Its origin is the tip
    *at the frame it was born* - reading the current tip instead is what made the trail snap back
    to the wand once a loop.
    """
    out = []
    count = 18
    for index in range(count):
        born_at = round(index * FRAMES / count) % FRAMES
        age = ((frame - born_at) % FRAMES) / FRAMES
        # Fade in fast, out slowly: a spark is brightest just after it appears, and the long tail
        # is what makes the trail read as a trail rather than as a ring around the tip.
        opacity = min(1.0, age * 6.0) * (1.0 - age)
        if opacity < 0.02:
            continue
        origin_x, origin_y = tip(born_at)
        # Deterministic jitter, so the sparks scatter instead of marching along fixed spokes.
        wobble = math.sin(index * 12.9898) * 18.0
        angle = math.radians(-206.0 + 76.0 * (index / (count - 1)) + wobble)
        drift = (175.0 + 65.0 * (index % 3) / 2.0) * age
        x = origin_x + drift * math.cos(angle)
        y = origin_y + drift * math.sin(angle)
        out.append(
            star(
                x,
                y,
                3.5 + (index % 4) * 1.7 + 6.0 * (1.0 - age),
                opacity,
                SPARKS[index % len(SPARKS)],
            )
        )
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
        f'{tip_x + 3 * ax:.1f},{tip_y + 3 * ay:.1f}" fill="#d8d2e8"/>'
    )
    highlight = (
        f'<polygon points="'
        f'{PIVOT[0] + 3 * ax:.1f},{PIVOT[1] + 3 * ay:.1f} '
        f'{PIVOT[0] - ax:.1f},{PIVOT[1] - ay:.1f} '
        f'{tip_x - 0.5 * ax:.1f},{tip_y - 0.5 * ay:.1f} '
        f'{tip_x + ax:.1f},{tip_y + ay:.1f}" fill="#ffffff" opacity="0.72"/>'
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
    # Small grip bands keep the wand itself detailed when the effects settle between bursts.
    bands = []
    along_x, along_y = tip_x - PIVOT[0], tip_y - PIVOT[1]
    for distance in (0.08, 0.18, 0.28):
        cx = PIVOT[0] - distance * along_x
        cy = PIVOT[1] - distance * along_y
        band = " ".join(
            f"{x:.1f},{y:.1f}"
            for x, y in (
                (cx + 10 * ax, cy + 10 * ay),
                (cx - 10 * ax, cy - 10 * ay),
                (cx - 7 * ax - 0.025 * along_x, cy - 7 * ay - 0.025 * along_y),
                (cx + 7 * ax - 0.025 * along_x, cy + 7 * ay - 0.025 * along_y),
            )
        )
        bands.append(f'<polygon points="{band}" fill="#c9a7ff" opacity="0.8"/>')

    # The tip glint pulses twice per loop, a beat ahead of the sweep.
    glint_pulse = 0.55 + 0.45 * math.sin(2.0 * math.pi * (frame / FRAMES) * 2.0)
    parts = [
        *trail(frame),
        star(tip_x, tip_y, 76.0 + 12.0 * glint_pulse, 0.08, "#8b5cff"),
        star(tip_x, tip_y, 58.0 + 10.0 * glint_pulse, 0.11, "#a8e6ff"),
        star(tip_x, tip_y, 43.0 + 8.0 * glint_pulse, 0.16, "#fff0a8"),
        *burst(frame, tip_x, tip_y),
        *sigils(frame, tip_x, tip_y),
        *sparkles(frame),
        handle,
        *bands,
        body,
        highlight,
        star(tip_x, tip_y, 26.0 + 10.0 * glint_pulse, 1.0, GLINT),
        star(tip_x, tip_y, 12.0, 1.0, "#fffdf5"),
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
    x = min(xs) - MARGIN
    y = min(ys) - MARGIN
    width = max(xs) - min(xs) + 2 * MARGIN
    height = max(ys) - min(ys) + 2 * MARGIN
    # Pad, never crop, to the exact terminal-cell aspect used by the plugin.
    if width / height > TARGET_ASPECT:
        padded = width / TARGET_ASPECT
        y -= (padded - height) / 2.0
        height = padded
    else:
        padded = height * TARGET_ASPECT
        x -= (padded - width) / 2.0
        width = padded
    return x, y, width, height


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
    print(f"placement: {DISPLAY_COLS}x{DISPLAY_ROWS} cells ({aspect / 0.5:.2f} columns per row)")


if __name__ == "__main__":
    main()
