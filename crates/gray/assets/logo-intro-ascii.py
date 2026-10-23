import math
import os
import re
import sys

from shapely.geometry import LineString, Point, Polygon
from shapely.ops import unary_union

BAND = 5.893
P = [(28.41797, 0.0), (118.27719, 0.0), (146.69458, 49.221035),
     (101.76586, 127.03985), (44.930503, 127.04018), (0.0, 49.221465)]
hull = Polygon(P)
CX, CY = sum(p[0] for p in P) / 6, sum(p[1] for p in P) / 6

SIDES = [(i, (i + 1) % 6) for i in range(6)]
DIAGS = [(0, 2), (0, 3), (0, 4), (1, 3), (1, 4), (1, 5), (2, 4), (2, 5), (3, 5)]
SHORT = (2.0840, 1.2885)

def offsets(i, j):
    k = (j - i) % 6
    if k == 3:
        return 0.0, 0.0
    if k in (1, 5):
        return BAND / 2, BAND / 2
    if k == 4:
        return offsets(j, i)[::-1]
    return SHORT if i % 2 == 0 else SHORT[::-1]

def stroke_seg(i, j):

    (x1, y1), (x2, y2) = P[i], P[j]
    L = math.dist(P[i], P[j])
    ux, uy = (x2 - x1) / L, (y2 - y1) / L
    nx, ny = -uy, ux
    if (CX - x1) * nx + (CY - y1) * ny < 0:
        nx, ny = -nx, -ny
    si, sj = offsets(i, j)
    A = (x1 + nx * si, y1 + ny * si)
    B = (x2 + nx * sj, y2 + ny * sj)
    l = math.dist(A, B)
    vx, vy = (B[0] - A[0]) / l, (B[1] - A[1]) / l
    seg = LineString([(A[0] - 20 * vx, A[1] - 20 * vy), (B[0] + 20 * vx, B[1] + 20 * vy)])
    clipped = seg.buffer(BAND / 2, cap_style="flat").intersection(hull)
    ts = [(x - A[0]) * vx + (y - A[1]) * vy for x, y in clipped.exterior.coords]
    a, b = min(ts), max(ts)
    return LineString([(A[0] + vx * a, A[1] + vy * a), (A[0] + vx * b, A[1] + vy * b)])

rim = list(hull.buffer(-BAND / 2, join_style="mitre").exterior.coords)[:-1]
k0 = min(range(6), key=lambda k: math.dist(rim[k], P[0]))
rim = rim[k0:] + rim[:k0]
if math.dist(rim[1], P[1]) > math.dist(rim[-1], P[1]):
    rim = rim[:1] + rim[:0:-1]

def toward(p, q, e=0.3):
    d = math.dist(p, q)
    return (p[0] + (q[0] - p[0]) * e / d, p[1] + (q[1] - p[1]) * e / d)

halves = [[rim[0], rim[1], rim[2], rim[3]], [rim[0], rim[5], rim[4], rim[3]]]
pens = [[toward(h[0], o), *h, toward(h[3], f)]
        for h, o, f in zip(halves, (rim[5], rim[1]), (rim[4], rim[2]))]

IN_OUT = (0.65, 0.0, 0.35, 1.0)
OUT = (0.22, 1.0, 0.36, 1.0)

def bezier_time(f, x1=0.65, y1=0.0, x2=0.35, y2=1.0):

    b = lambda s, p1, p2: 3 * (1 - s) ** 2 * s * p1 + 3 * (1 - s) * s * s * p2 + s ** 3
    lo, hi = 0.0, 1.0
    for _ in range(40):
        m = (lo + hi) / 2
        lo, hi = (m, hi) if b(m, y1, y2) < f else (lo, m)
    return b(lo, x1, x2)

def bezier_eval(x, p):

    x1, y1, x2, y2 = p
    bx = lambda s: 3 * (1 - s) ** 2 * s * x1 + 3 * (1 - s) * s * s * x2 + s ** 3
    by = lambda s: 3 * (1 - s) ** 2 * s * y1 + 3 * (1 - s) * s * s * y2 + s ** 3
    lo, hi = 0.0, 1.0
    for _ in range(40):
        m = (lo + hi) / 2
        lo, hi = (m, hi) if bx(m) < x else (lo, m)
    return max(0.0, min(1.0, by((lo + hi) / 2)))

reach = {0: 0.0, 3: 1.25}
for h in halves:
    ls = LineString(h)
    for v, pt in zip((1, 2), h[1:3]):
        reach[[i for i in range(6) if math.dist(rim[i], pt) < 1e-9][0]] = (
            1.25 * bezier_time(ls.project(Point(pt)) / ls.length)
        )
diags = sorted(((i, j) if reach[i] <= reach[j] else (j, i) for i, j in DIAGS),
               key=lambda e: reach[e[0]])

T0, SIDE_DUR, DIAG_DUR = 0.15, 1.25, 0.85
elements = []
for pen in pens:
    elements.append((LineString(pen), T0, SIDE_DUR, IN_OUT, "mitre"))
for i, j in diags:
    elements.append((stroke_seg(i, j), T0 + reach[i], DIAG_DUR, OUT, "mitre"))

FPS = 30
T_END = max(start + dur for _, start, dur, _, _ in elements)
N_FRAMES = int(T_END * FPS) + 1

CELLS_X, CELLS_Y = 32, 12
# Tip-flag radius in dot units, scaled with the grid
# (1.5 dots was tuned at 16 cells wide).
EDGE_R = 1.5 * CELLS_X / 16
X0, X1 = min(p[0] for p in P), max(p[0] for p in P)
Y0, Y1 = min(p[1] for p in P), max(p[1] for p in P)
DOT_X = (X1 - X0) / (CELLS_X * 2)
DOT_Y = (Y1 - Y0) / (CELLS_Y * 4)

BITS = {(0, 0): 0x01, (0, 1): 0x02, (0, 2): 0x04, (0, 3): 0x40,
        (1, 0): 0x08, (1, 1): 0x10, (1, 2): 0x20, (1, 3): 0x80}

def winding_cover(subpaths):

    polys = []
    for sp in subpaths:
        if len(sp) < 3:
            continue
        sign = 1 if sum(sp[i][0] * sp[i + 1][1] - sp[i + 1][0] * sp[i][1]
                        for i in range(len(sp) - 1)) / 2 > 0 else -1
        polys.append((Polygon(sp), sign))

    def covers(pt):
        return sum(s for p, s in polys if p.covers(pt)) != 0

    return covers

def rasterize(geom):
    return rasterize_pred(geom.covers)

def rasterize_pred(cover_fn):

    rows = []
    for row in range(CELLS_Y):
        line = []
        for col in range(CELLS_X):
            bits = 0
            for dx in range(2):
                for dy in range(4):
                    x = X0 + (col * 2 + dx) * DOT_X
                    y = Y0 + (row * 4 + dy) * DOT_Y
                    n = 0
                    for sy in range(3):
                        for sx in range(3):
                            pt = Point(x + (sx + 0.5) / 3 * DOT_X,
                                       y + (sy + 0.5) / 3 * DOT_Y)
                            if cover_fn(pt):
                                n += 1
                    if n >= 5:
                        bits |= BITS[(dx, dy)]
            line.append(chr(0x2800 + bits))
        rows.append("".join(line))
    return rows

def _partial(path, dist):

    yield 0.0
    acc = 0.0
    coords = list(path.coords)
    for a, b in zip(coords, coords[1:]):
        acc += math.dist(a, b)
        if acc >= dist:
            break
        yield acc
    yield dist

def frame(t):
    geoms, tips = [], []
    for path, start, dur, ease, join in elements:
        frac = bezier_eval(min(max((t - start) / dur, 0.0), 1.0), ease)
        if frac < 1e-6:
            continue
        if frac > 1.0 - 1e-6:
            frac = 1.0
        length = min(frac, 1.0) * path.length
        seg = LineString(path.interpolate(d) for d in _partial(path, length))
        geoms.append(seg.buffer(BAND / 2, cap_style="flat",
                                join_style=join).intersection(hull))
        if frac < 1.0:
            tips.append(path.interpolate(length))
    union = unary_union(geoms) if geoms else Polygon()
    return union, tips

def edge_rows(tips):

    flag = [[False] * CELLS_X for _ in range(CELLS_Y)]
    for tip in tips:
        gx = (tip.x - X0) / DOT_X
        gy = (tip.y - Y0) / DOT_Y
        for row in range(CELLS_Y):
            for col in range(CELLS_X):
                for dx in range(2):
                    for dy in range(4):
                        px = col * 2 + dx + 0.5
                        py = row * 4 + dy + 0.5
                        if math.hypot(px - gx, py - gy) <= EDGE_R:
                            flag[row][col] = True
    out = []
    for row in range(CELLS_Y):
        out.append("".join("1" if flag[row][col] else "0"
                           for col in range(CELLS_X)))
    return out

def main():
    frames = []
    for n in range(N_FRAMES):
        union, tips = frame(n / FPS)
        mark = rasterize(union) if not union.is_empty else ["" * CELLS_X] * CELLS_Y
        frames.append((mark, edge_rows(tips)))

    full = unary_union([
        LineString(pen).buffer(BAND / 2, cap_style="flat",
                               join_style="mitre").intersection(hull)
        for pen in pens
    ] + [
        stroke_seg(i, j).buffer(BAND / 2, cap_style="flat").intersection(hull)
        for i, j in diags
    ])
    final = rasterize(full)

    here = os.path.dirname(os.path.abspath(__file__))
    ref = os.path.join(here, "logo.txt")
    if "--write-logo" in sys.argv:
        rows = [r.rstrip("\u2800") for r in final]
        while rows and not rows[-1]:
            rows.pop()
        with open(ref, "w") as f:
            f.write("\n".join(rows) + "\n")
        print(f"wrote {ref}: {len(rows)} rows")
    if os.path.exists(ref):
        old = [l.lstrip(" ") for l in open(ref).read().splitlines()]
        old = [l + "\u2800" * (CELLS_X - len(l)) for l in old]
        old += ["\u2800" * CELLS_X] * (CELLS_Y - len(old))
        diffs = sum(a != b for ra, rb in zip(final, old) for a, b in zip(ra, rb))
        print(f"band-union final vs logo.txt: {diffs} cell(s) differ")
        frames[-1] = (old, ["0" * CELLS_X] * CELLS_Y)
        assert frames[-1][0] == old

    out = os.path.join(here, "logo-intro-frames.txt")
    with open(out, "w") as f:
        f.write(f"# gray mark intro, {N_FRAMES} frames at {FPS}fps — "
                "baked by logo-intro-ascii.py from logo-intro-pens.py\n")
        for mark, edge in frames:
            f.write("\n".join(mark) + "\n" + "\n".join(edge) + "\n---\n")

    dark = os.path.expanduser("~/content/gray/logo/logo-dark.svg")
    if os.path.exists(dark):
        try:
            ref_rows = rasterize_pred(winding_cover(dark_svg_subpaths(dark)))
            diffs = sum(a != b for ra, rb in zip(final, ref_rows)
                        for a, b in zip(ra, rb))
            print(f"final vs logo-dark.svg: {diffs} cell(s) differ")
            if diffs:
                for r, (a, b) in enumerate(zip(final, ref_rows)):
                    if a != b:
                        print(f"  row {r}: baked {a!r} vs svg {b!r}")
        except Exception as e:
            print(f"logo-dark.svg parse skipped: {e}")

    print(f"wrote {out}: {N_FRAMES} frames, {T_END:.2f}s at {FPS}fps")

def dark_svg_subpaths(path):

    d = re.search(r'<path fill="#ffffff"[^>]*d="([^"]+)"',
                  open(path).read()).group(1)
    toks = re.findall(r'[MLlZzMm]|-?[\d.eE+-]+', d.replace(",", " "))
    subpaths, cur, i, pen, start = [], [], 0, (0.0, 0.0), (0.0, 0.0)
    while i < len(toks):
        cmd = toks[i]
        if cmd in ("M", "L", "m", "l"):
            x, y = float(toks[i + 1]), float(toks[i + 2])
            if cmd.islower():
                x, y = pen[0] + x, pen[1] + y
            pen = (x, y)
            if cmd in ("M", "m"):
                if cur:
                    subpaths.append(cur)
                cur = [pen]
                start = pen
            else:
                cur.append(pen)
            i += 3

            while i < len(toks) and toks[i] not in "MLlZzMm":
                x, y = float(toks[i]), float(toks[i + 1])
                if cmd.islower():
                    x, y = pen[0] + x, pen[1] + y
                pen = (x, y)
                cur.append(pen)
                i += 2
        elif cmd in ("Z", "z"):
            pen = start
            i += 1
        else:
            i += 1
    if cur:
        subpaths.append(cur)
    return subpaths

if __name__ == "__main__":
    main()
