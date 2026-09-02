#!/usr/bin/env python3
"""Download the three SIL OFL 1.1 faces with their licenses.

Run from web/:  python3 scripts/fetch-fonts.py

public/fonts/ holds the woff2 the browser downloads. Instrument Serif (display)
and Departure Mono (labels) ship as static woff2 from their upstream repos;
Newsreader (body) is pulled as the latin-subset variable woff2 that Google
serves, so we self-host instead of hot-linking gstatic.

assets/fonts/ holds ttf/otf for the two faces the OG card draws with. satori
(next/og) rejects the wOF2 signature outright, and these are read at build time
only — keeping them out of public/ means they are never served.
"""
import os
import re
import urllib.request

OUT = "public/fonts"
BUILD_OUT = "assets/fonts"
UA = {"User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120 Safari/537.36"}

INSTRUMENT = "https://raw.githubusercontent.com/Instrument/instrument-serif/main"
DEPARTURE = "https://raw.githubusercontent.com/rektdeckard/departure-mono/main/public/assets"

# served to the browser
FILES = {
    "InstrumentSerif-Regular.woff2": f"{INSTRUMENT}/fonts/webfonts/InstrumentSerif-Regular.woff2",
    "InstrumentSerif-Italic.woff2": f"{INSTRUMENT}/fonts/webfonts/InstrumentSerif-Italic.woff2",
    "OFL-InstrumentSerif.txt": f"{INSTRUMENT}/OFL.txt",
    "DepartureMono-Regular.woff2": f"{DEPARTURE}/DepartureMono-Regular.woff2",
    "OFL-DepartureMono.txt": f"{DEPARTURE}/LICENSE",
    "OFL-Newsreader.txt": "https://raw.githubusercontent.com/productiontype/Newsreader/master/OFL.txt",
}

# read at build time by app/opengraph-image.tsx, never served.
# OFL 1.1 requires the license to accompany every redistributed copy, so these
# get their own license files rather than relying on the ones under public/.
BUILD_FILES = {
    "InstrumentSerif-Regular.ttf": f"{INSTRUMENT}/fonts/ttf/InstrumentSerif-Regular.ttf",
    "OFL-InstrumentSerif.txt": f"{INSTRUMENT}/OFL.txt",
    "DepartureMono-Regular.otf": f"{DEPARTURE}/DepartureMono-Regular.otf",
    "OFL-DepartureMono.txt": f"{DEPARTURE}/LICENSE",
}

NEWSREADER_CSS = "https://fonts.googleapis.com/css2?family=Newsreader:opsz,wght@6..72,200..700&display=swap"


def get(url: str, dest: str) -> None:
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=60) as r:
        data = r.read()
    with open(dest, "wb") as f:
        f.write(data)
    print(f"{os.path.basename(dest):34} {len(data) // 1024:>5} KB")


def main() -> None:
    os.makedirs(OUT, exist_ok=True)
    os.makedirs(BUILD_OUT, exist_ok=True)
    for name, url in FILES.items():
        get(url, os.path.join(OUT, name))
    for name, url in BUILD_FILES.items():
        get(url, os.path.join(BUILD_OUT, name))

    req = urllib.request.Request(NEWSREADER_CSS, headers=UA)
    with urllib.request.urlopen(req, timeout=60) as r:
        css = r.read().decode()
    # each @font-face is preceded by a /* subset */ comment; we only want latin
    for subset, url in re.findall(r"/\* (\S+) \*/\s*@font-face \{[^}]*?src: url\((\S+?)\)", css, re.S):
        if subset == "latin":
            get(url, os.path.join(OUT, "Newsreader-Variable.woff2"))
            break
    else:
        raise SystemExit("could not find the latin subset in the Newsreader CSS")


if __name__ == "__main__":
    main()
