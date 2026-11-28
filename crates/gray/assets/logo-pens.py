import json
import math
import os

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

T_END = max(start + dur for _, start, dur, _, _ in elements)

def main():
    out = {
        "band": BAND,
        "hull": [[round(x, 4), round(y, 4)] for x, y in P],
        "end": round(T_END, 4),
        "strokes": [
            {
                "path": [[round(x, 4), round(y, 4)] for x, y in path.coords],
                "start": round(start, 4),
                "dur": dur,
                "ease": list(ease),
            }
            for path, start, dur, ease, _ in elements
        ],
    }
    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "logo-pens.json"), "w") as f:
        json.dump(out, f, separators=(",", ":"))
        f.write("\n")

if __name__ == "__main__":
    main()
