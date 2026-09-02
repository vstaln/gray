#!/usr/bin/env python3
"""Render the gray mark into the site's favicon and apple-touch icon.

Source is docs/assets/gray-logo-clean.svg (black on transparent); the site
inverts it to paper-on-ink so it stays legible in dark browser chrome.

The mark is fine line-art with many internal facets. Below ~48px those strokes
fall between pixels and it reads as grey mush, so small frames instead use the
mark's own outer hexagon silhouette — same geometry, legible at tab size.

Run from web/:  python3 scripts/make-icons.py
"""
import io
import math
import os

import cairosvg
from PIL import Image, ImageDraw

SRC = "../docs/assets/gray-logo-clean.svg"
INK = (5, 5, 6, 255)
PAPER = "#eceae7"
PAPER_RGB = (236, 234, 231, 255)
DETAIL_MIN = 48  # below this, use the silhouette
PAD = 0.14


def _canvas(size: int) -> Image.Image:
    return Image.new("RGBA", (size, size), INK)


def render_detailed(size: int) -> Image.Image:
    svg = open(SRC, encoding="utf-8").read().replace('fill="black"', f'fill="{PAPER}"')
    inner = int(size * (1 - 2 * PAD))
    png = cairosvg.svg2png(bytestring=svg.encode(), output_width=inner, output_height=inner)
    mark = Image.open(io.BytesIO(png)).convert("RGBA")
    canvas = _canvas(size)
    canvas.paste(mark, ((size - mark.width) // 2, (size - mark.height) // 2), mark)
    return canvas


def render_silhouette(size: int) -> Image.Image:
    """The mark's outer hexagon, filled. Drawn at 8x and downsampled for edges."""
    ss = size * 8
    canvas = _canvas(ss)
    d = ImageDraw.Draw(canvas)
    r = ss * 0.40
    cx = cy = ss / 2
    # flat-top hexagon matching the source mark's orientation
    pts = [
        (cx + r * math.cos(math.radians(a)), cy + r * math.sin(math.radians(a)))
        for a in range(-90, 270, 60)
    ]
    d.polygon(pts, fill=PAPER_RGB)
    return canvas.resize((size, size), Image.LANCZOS)


def render(size: int) -> Image.Image:
    return render_detailed(size) if size >= DETAIL_MIN else render_silhouette(size)


def main() -> None:
    sizes = [16, 32, 48, 64, 128, 256]
    frames = {s: render(s) for s in sizes}
    # Pillow only embeds a supplied frame when it is handed the exact size via
    # append_images; otherwise it silently downscales the base image and the
    # small frames lose the silhouette.
    largest = frames[max(sizes)]
    largest.save(
        "app/favicon.ico",
        format="ICO",
        sizes=[(s, s) for s in sizes],
        append_images=[frames[s] for s in sizes if s != max(sizes)],
    )
    render_detailed(180).convert("RGB").save("app/apple-icon.png")
    print("app/favicon.ico", "app/apple-icon.png", sep="\n")


if __name__ == "__main__":
    main()
