#!/usr/bin/env python3
"""Regenerate with Python 3, pycairo, Pillow, fontconfig, and rsvg-convert.

Run `python3 assets/wordmark-explorations/generate.py` from the repo root.
The five bold font families below must be installed; font files are not bundled.
SVGs have 200-unit visible glyph height and 40-unit padding; PNGs render at 2x.
"""

from dataclasses import dataclass
from html import escape
import math
from pathlib import Path
import shutil
import subprocess

import cairo
from PIL import Image, ImageDraw, ImageFont


OUTPUT = Path(__file__).resolve().parent
WORD = "yinz"
GLYPH_HEIGHT = 200
PADDING = 40
PNG_SCALE = 2


@dataclass(frozen=True)
class Font:
    slug: str
    family: str
    label: str
    detail: str


FONTS = (
    Font("dejavu-sans-bold", "DejaVu Sans", "DejaVu Sans", "Bold"),
    Font("ubuntu-bold", "Ubuntu Nerd Font", "Ubuntu", "Nerd Font / Bold"),
    Font("noto-serif-bold", "Noto Serif", "Noto Serif", "Bold"),
    Font(
        "jetbrains-mono-bold",
        "JetBrainsMono Nerd Font",
        "JetBrains Mono",
        "Nerd Font / Bold",
    ),
    Font(
        "ibm-plex-mono-bold",
        "BlexMono Nerd Font",
        "IBM Plex Mono",
        "Blex Mono Nerd Font / Bold",
    ),
)


def resolve_font(family: str, style: str = "Bold") -> Path:
    """Reject fontconfig's silent substitution if a requested face is missing."""
    result = subprocess.run(
        ["fc-match", "-f", "%{family}\n%{style}\n%{file}\n", f"{family}:style={style}"],
        check=True,
        capture_output=True,
        text=True,
    )
    families, matched_style, filename = result.stdout.strip().splitlines()
    if family not in families.split(",") or style not in matched_style.split(","):
        raise RuntimeError(
            f"Install {family} {style}; fontconfig resolved {families} {matched_style}"
        )
    return Path(filename)


def write_wordmark(font: Font) -> Path:
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, 1, 1)
    context = cairo.Context(surface)
    context.select_font_face(font.family, cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_BOLD)
    context.set_font_size(1000)
    options = cairo.FontOptions()
    options.set_hint_style(cairo.HINT_STYLE_NONE)
    options.set_hint_metrics(cairo.HINT_METRICS_OFF)
    context.set_font_options(options)
    context.move_to(0, 0)
    context.text_path(WORD)
    left, top, right, bottom = context.path_extents()
    if right <= left or bottom <= top:
        raise RuntimeError(f"Empty glyph outlines for {font.family}")

    scale = GLYPH_HEIGHT / (bottom - top)
    visible_width = (right - left) * scale
    width = math.ceil(visible_width + 2 * PADDING)
    height = GLYPH_HEIGHT + 2 * PADDING
    offset_x = (width - visible_width) / 2
    commands = []
    command_names = {
        cairo.PATH_MOVE_TO: "M",
        cairo.PATH_LINE_TO: "L",
        cairo.PATH_CURVE_TO: "C",
        cairo.PATH_CLOSE_PATH: "Z",
    }
    for kind, points in context.copy_path():
        coordinates = []
        for index in range(0, len(points), 2):
            x = (points[index] - left) * scale + offset_x
            y = (points[index + 1] - top) * scale + PADDING
            coordinates.extend((f"{x:.4f}", f"{y:.4f}"))
        commands.append(command_names[kind] + " ".join(coordinates))

    svg_path = OUTPUT / f"yinz-{font.slug}.svg"
    svg_path.write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<svg xmlns="http://www.w3.org/2000/svg" '
        f'width="{width}" height="{height}" viewBox="0 0 {width} {height}" '
        'role="img" aria-labelledby="title description">\n'
        f'  <title id="title">yinz — {escape(font.label)} Bold</title>\n'
        '  <desc id="description">Lowercase yinz converted to filled glyph contours '
        f'from {escape(font.family)} Bold. Transparent background.</desc>\n'
        f'  <path fill="#000000" d="{" ".join(commands)}"/>\n'
        '</svg>\n',
        encoding="utf-8",
    )
    png_path = svg_path.with_suffix(".png")
    subprocess.run(
        [
            "rsvg-convert",
            "--width", str(width * PNG_SCALE),
            "--height", str(height * PNG_SCALE),
            "--output", str(png_path),
            str(svg_path),
        ],
        check=True,
    )
    print(f"{svg_path.name}: {width}×{height}; PNG: {width * PNG_SCALE}×{height * PNG_SCALE}")
    return png_path


def write_comparison(previews: list[Path], label_font: Path) -> None:
    width, margin, header_height, row_height = 1200, 64, 150, 248
    height = header_height + len(FONTS) * row_height + 64
    sheet = Image.new("RGB", (width, height), "white")
    draw = ImageDraw.Draw(sheet)
    title = ImageFont.truetype(str(label_font), 36)
    label = ImageFont.truetype(str(label_font), 25)
    small = ImageFont.truetype(str(label_font), 17)
    draw.text((margin, 40), "yinz / wordmark studies", font=title, fill="#171717")
    draw.text(
        (margin, 96), "Five bold fonts / lowercase outlines / equal glyph height",
        font=small, fill="#666666",
    )
    for index, (font, preview_path) in enumerate(zip(FONTS, previews, strict=True)):
        top = header_height + index * row_height
        draw.line((margin, top, width - margin, top), fill="#dddddd")
        draw.text((margin, top + 45), f"0{index + 1}", font=small, fill="#777777")
        draw.text((margin, top + 88), font.label, font=label, fill="#171717")
        draw.text((margin, top + 127), font.detail, font=small, fill="#666666")
        with Image.open(preview_path) as preview:
            # 400-pixel glyphs in the 2x previews become 152-pixel glyphs here.
            thumbnail = preview.convert("RGBA").resize(
                (round(preview.width * 0.38), round(preview.height * 0.38)),
                Image.Resampling.LANCZOS,
            )
        sheet.paste(thumbnail, (470, top + (row_height - thumbnail.height) // 2), thumbnail)
    sheet.save(OUTPUT / "comparison.png")
    print(f"comparison.png: {width}×{height}")


def main() -> None:
    for command in ("fc-match", "rsvg-convert"):
        if shutil.which(command) is None:
            raise RuntimeError(f"Required command is missing: {command}")
    # Resolve everything before writing so missing families do not leave a partial set.
    font_paths = [resolve_font(font.family) for font in FONTS]
    previews = [write_wordmark(font) for font in FONTS]
    write_comparison(previews, font_paths[0])


if __name__ == "__main__":
    main()
