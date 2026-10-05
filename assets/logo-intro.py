"""gray mark (K6) logo intro, Google-2015 style, looping.

Everything leaves the top-left corner: two pens trace the rim to bottom-right,
and each diagonal sets off from its corner as a pen passes -> mark eases up ->
hold -> lines unwind, mark eases back and fades -> loop. Transparent background.

Geometry (checked against a render of logo-dark.svg): every band is 5.893 wide;
sides lie inside the hull, long diagonals are centred on the vertex line, short
diagonals are slightly tilted (see SHORT). Each
band is one butt-capped stroke spanning exactly the projection of band ∩ hull
onto its centreline, under a hull clip-path (shapely approach of
grayspace/assets/icons/slate-logo.py).

Run: python3 logo-intro.py [snapdir]  -> logo-intro.svg (+ paused
snapshot SVGs in snapdir).
"""
import math, sys
from shapely.geometry import LineString, Point, Polygon

BAND = 5.893
P = [(28.41797, 0.0), (118.27719, 0.0), (146.69458, 49.221035),   # hull from logo-dark.svg
     (101.76586, 127.03985), (44.930503, 127.04018), (0.0, 49.221465)]
hull = Polygon(P)
CX, CY = sum(p[0] for p in P) / 6, sum(p[1] for p in P) / 6
TX, TY, SCALE = 84, 138, 5.46

SIDES = [(i, (i + 1) % 6) for i in range(6)]                        # round the rim
DIAGS = [(0, 2), (0, 3), (0, 4), (1, 3), (1, 4), (1, 5), (2, 4), (2, 5), (3, 5)]


# Short diagonals (i, i+2) are not parallel to their vertex line: fitted from
# logo-dark.svg (fit4.py, residual < 0.002), the centre sits 2.0840 inside the
# line at one end and 1.2885 at the other, in a pinwheel (even-vertex triangle
# wide at its start, odd one at its end).
SHORT = (2.0840, 1.2885)


def offsets(i, j):
    k = (j - i) % 6
    if k == 3:
        return 0.0, 0.0                                              # long: centred
    if k in (1, 5):
        return BAND / 2, BAND / 2                                    # side: inside hull
    if k == 4:
        return offsets(j, i)[::-1]
    return SHORT if i % 2 == 0 else SHORT[::-1]


def stroke(i, j):
    (x1, y1), (x2, y2) = P[i], P[j]
    L = math.dist(P[i], P[j])
    ux, uy = (x2 - x1) / L, (y2 - y1) / L
    nx, ny = -uy, ux
    if (CX - x1) * nx + (CY - y1) * ny < 0:
        nx, ny = -nx, -ny                                            # normal toward centre
    si, sj = offsets(i, j)
    A = (x1 + nx * si, y1 + ny * si)
    B = (x2 + nx * sj, y2 + ny * sj)
    l = math.dist(A, B)
    vx, vy = (B[0] - A[0]) / l, (B[1] - A[1]) / l
    seg = LineString([(A[0] - 20 * vx, A[1] - 20 * vy), (B[0] + 20 * vx, B[1] + 20 * vy)])
    clipped = seg.buffer(BAND / 2, cap_style="flat").intersection(hull)
    ts = [(x - A[0]) * vx + (y - A[1]) * vy for x, y in clipped.exterior.coords]
    a, b = min(ts), max(ts)
    return f"M {A[0] + vx * a:.4f} {A[1] + vy * a:.4f} L {A[0] + vx * b:.4f} {A[1] + vy * b:.4f}"


# ---- timeline (seconds). Each group shares one keyframe; per-element --d staggers it.
T = 6.0
pct = lambda t: f"{100 * t / T:.2f}%"
IN_OUT = "cubic-bezier(0.65, 0, 0.35, 1)"      # pen: eases in, cruises, eases out
OUT = "cubic-bezier(0.22, 1, 0.36, 1)"         # quint out: quick start, long glide
IN = "cubic-bezier(0.55, 0, 0.75, 0.2)"        # gentle accelerate away


def line_kf(name, draw, dur, ease, retract, rdur):
    return (f"    @keyframes {name} {{ 0%, {pct(draw)} {{ stroke-dashoffset: 1; animation-timing-function: {ease}; }} "
            f"{pct(draw + dur)}, {pct(retract)} {{ stroke-dashoffset: 0; animation-timing-function: {IN_OUT}; }} "
            f"{pct(retract + rdur)}, 100% {{ stroke-dashoffset: -1; }} }}")


kfs = [
    line_kf("side", 0.15, 1.25, IN_OUT, 3.9, 1.1),    # two pens round the rim, top-left -> bottom-right
    line_kf("diag", 0.15, 0.85, OUT, 3.75, 0.6),      # +--d: each leaves its corner as a rim pen passes
    # whole mark: drifts up to size while drawing, eases back and fades on the way out
    f"    @keyframes settle {{ 0% {{ transform: scale(0.95); opacity: 1; animation-timing-function: cubic-bezier(0.33, 1, 0.68, 1); }} "
    f"{pct(2.6)}, {pct(3.6)} {{ transform: scale(1); opacity: 1; animation-timing-function: {IN}; }} "
    f"{pct(5.1)} {{ opacity: 1; }} "
    f"{pct(5.5)}, 100% {{ transform: scale(0.97); opacity: 0; }} }}",
]

# Rim: the inset hexagon as two pens leaving the top-left corner, one each way,
# meeting at bottom-right. Each runs a hair past both corners so its miter joins
# fill them.
rim = list(hull.buffer(-BAND / 2, join_style="mitre").exterior.coords)[:-1]
k0 = min(range(6), key=lambda k: math.dist(rim[k], P[0]))
rim = rim[k0:] + rim[:k0]                                            # rim[0] ~ top-left
if math.dist(rim[1], P[1]) > math.dist(rim[-1], P[1]):
    rim = rim[:1] + rim[:0:-1]                                       # clockwise, like P
toward = lambda p, q, e=0.3: (p[0] + (q[0] - p[0]) * e / math.dist(p, q), p[1] + (q[1] - p[1]) * e / math.dist(p, q))
halves = [[rim[0], rim[1], rim[2], rim[3]], [rim[0], rim[5], rim[4], rim[3]]]
pens = [[toward(h[0], o), *h, toward(h[3], f)] for h, o, f in zip(halves, (rim[5], rim[1]), (rim[4], rim[2]))]


def bezier_time(f, x1=0.65, y1=0.0, x2=0.35, y2=1.0):
    """Time fraction at which IN_OUT reaches progress f."""
    b = lambda s, p1, p2: 3 * (1 - s) ** 2 * s * p1 + 3 * (1 - s) * s * s * p2 + s ** 3
    lo, hi = 0.0, 1.0
    for _ in range(40):
        m = (lo + hi) / 2
        lo, hi = (m, hi) if b(m, y1, y2) < f else (lo, m)
    return b(lo, x1, x2)


# When a rim pen reaches each corner; a diagonal leaves from whichever end is reached first.
reach = {0: 0.0, 3: 1.25}
for h in halves:
    ls = LineString(h)
    for v, pt in zip((1, 2), h[1:3]):
        reach[[i for i in range(6) if math.dist(rim[i], pt) < 1e-9][0]] = 1.25 * bezier_time(ls.project(Point(pt)) / ls.length)
diags = sorted(((i, j) if reach[i] <= reach[j] else (j, i) for i, j in DIAGS), key=lambda e: reach[e[0]])
lines = [f'        <path class="b side" pathLength="1" stroke-linejoin="miter" d="M {" L ".join(f"{x:.4f} {y:.4f}" for x, y in p)}" />'
         for p in pens]
lines += [f'        <path class="b diag" style="--d:{reach[i]:.3f}s" pathLength="1" d="{stroke(i, j)}" />'
          for i, j in diags]
hull_d = "M " + " L ".join(f"{x} {y}" for x, y in P) + " Z"
ox, oy = TX + CX * SCALE, TY + CY * SCALE


def svg(extra=""):
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="970" height="970" viewBox="0 0 970 970">
  <title>gray mark intro</title>
  <defs><clipPath id="hull"><path d="{hull_d}" /></clipPath></defs>
  <g class="mark">
    <g transform="translate({TX} {TY}) scale({SCALE})">
      <g clip-path="url(#hull)" fill="none" stroke="#ffffff" stroke-width="{BAND}" stroke-linecap="butt">
{chr(10).join(lines)}
      </g>
    </g>
  </g>
  <style>
    .b {{ stroke-dasharray: 1 1; }}
    .mark {{ transform-origin: {ox:.2f}px {oy:.2f}px; }}
    .side, .diag, .mark {{ animation: {T}s linear var(--d, 0s) infinite both; }}
    .side {{ animation-name: side; }}
    .diag {{ animation-name: diag; }}
    .mark {{ animation-name: settle; }}
{chr(10).join(kfs)}
    @media (prefers-reduced-motion: reduce) {{
      .side, .diag, .mark {{ animation: none; }}
    }}{extra}
  </style>
</svg>
"""


open(__file__.replace(".py", ".svg"), "w").write(svg())

if len(sys.argv) > 1:  # frozen moments for visual checks
    moments = {"1-sides": 0.6, "2-diags": 1.1, "3-settle": 2.0,
               "4-full": 3.2, "5-retract": 4.3, "6-empty": 5.8}
    for name, at in moments.items():
        open(f"{sys.argv[1]}/snap-{name}.svg", "w").write(svg(
            f"\n    .side, .diag, .mark {{ animation-delay: calc(var(--d, 0s) - {at}s) !important;"
            f" animation-play-state: paused !important; }}"))
    open(f"{sys.argv[1]}/snap-static.svg", "w").write(svg(
        "\n    .side, .diag, .mark { animation: none !important; }"))
