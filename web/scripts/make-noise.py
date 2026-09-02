#!/usr/bin/env python3
"""Generate the 128x128 tiling luminance noise used by the .grain utility.

Run from web/:  python3 scripts/make-noise.py
"""
import numpy as np
from PIL import Image

rng = np.random.default_rng(7)
n = rng.normal(128, 34, (128, 128)).clip(0, 255).astype(np.uint8)
Image.fromarray(n, mode="L").save("public/space/noise.png", optimize=True)
print("public/space/noise.png")
