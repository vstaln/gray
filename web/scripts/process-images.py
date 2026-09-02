#!/usr/bin/env python3
"""Bake NASA source plates into the site's two image treatments.

  dither  — Floyd-Steinberg 1-bit halftone on black, the Hermes pricing-card look
  plate   — desaturated + contrast-lifted photo for full-bleed sections

Source images are NASA public domain (see public/space/CREDITS.md).
Run from web/:  python3 scripts/process-images.py ../../nasa/raw
"""
import sys, os
from PIL import Image, ImageEnhance, ImageOps
import numpy as np

SRC = sys.argv[1] if len(sys.argv) > 1 else "/tmp/nasa/raw"
OUT = "public/space"
os.makedirs(OUT, exist_ok=True)

def load(name):
    return Image.open(f"{SRC}/{name}.jpg").convert("RGB")

def cover(im, w, h):
    sr, tr = im.width / im.height, w / h
    if sr > tr:
        nh = h; nw = int(h * sr)
    else:
        nw = w; nh = int(w / sr)
    im = im.resize((nw, nh), Image.LANCZOS)
    return im.crop(((nw - w) // 2, (nh - h) // 2, (nw - w) // 2 + w, (nh - h) // 2 + h))

def dither(name, w, h, gamma=1.0, out=None):
    """1-bit Floyd-Steinberg. Reads as engraving at small sizes, grain at large."""
    im = cover(load(name), w, h).convert("L")
    im = ImageEnhance.Contrast(im).enhance(1.35)
    if gamma != 1.0:
        a = np.asarray(im).astype(np.float32) / 255.0
        im = Image.fromarray((np.power(a, gamma) * 255).astype(np.uint8))
    im.convert("1").save(f"{OUT}/{out or name}-dither.png", optimize=True)

def plate(name, w, h, sat=0.14, bright=0.72, out=None):
    """Near-monochrome photographic plate that sits under text."""
    im = cover(load(name), w, h)
    im = ImageEnhance.Color(im).enhance(sat)
    im = ImageEnhance.Brightness(im).enhance(bright)
    im = ImageEnhance.Contrast(im).enhance(1.18)
    im.save(f"{OUT}/{out or name}-plate.jpg", quality=82, optimize=True, progressive=True)

# hero + full-bleed plates
plate("carina", 2400, 1400, sat=0.10, bright=0.55)
plate("pillars", 1800, 1200, sat=0.12, bright=0.60)
plate("andromeda", 2400, 1000, sat=0.10, bright=0.50)
plate("earthlimb", 2400, 900, sat=0.16, bright=0.65)
plate("mwcore", 2000, 1200, sat=0.10, bright=0.55)

# dithered engravings — pricing cards, panels, footer
for n in ("helix", "saturn", "jupiter", "eclipse", "moon", "aurora", "bluemarble", "carina"):
    dither(n, 900, 520)
dither("pillars", 1400, 900, gamma=1.15)
dither("andromeda", 1400, 700)

print("\n".join(sorted(os.listdir(OUT))))
