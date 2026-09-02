#!/usr/bin/env python3
"""Download the three SIL OFL 1.1 faces into public/fonts/ with their licenses.

Run from web/:  python3 scripts/fetch-fonts.py

Instrument Serif (display) and Departure Mono (labels) ship as static woff2 from
their upstream repos. Newsreader (body) is pulled as the latin-subset variable
woff2 that Google serves, so we self-host instead of hot-linking gstatic.
"""
import os
import re
import urllib.request

OUT = "public/fonts"
UA = {"User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120 Safari/537.36"}

FILES = {
    "InstrumentSerif-Regular.woff2": "https://raw.githubusercontent.com/Instrument/instrument-serif/main/fonts/webfonts/InstrumentSerif-Regular.woff2",
    "InstrumentSerif-Italic.woff2": "https://raw.githubusercontent.com/Instrument/instrument-serif/main/fonts/webfonts/InstrumentSerif-Italic.woff2",
    "OFL-InstrumentSerif.txt": "https://raw.githubusercontent.com/Instrument/instrument-serif/main/OFL.txt",
    "DepartureMono-Regular.woff2": "https://raw.githubusercontent.com/rektdeckard/departure-mono/main/public/assets/DepartureMono-Regular.woff2",
    "OFL-DepartureMono.txt": "https://raw.githubusercontent.com/rektdeckard/departure-mono/main/public/assets/LICENSE",
    "OFL-Newsreader.txt": "https://raw.githubusercontent.com/productiontype/Newsreader/master/OFL.txt",
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
    for name, url in FILES.items():
        get(url, os.path.join(OUT, name))

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
