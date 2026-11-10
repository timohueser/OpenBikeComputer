"""Prototype: the climb of each recorded ride over the road heights of the TrailView solver.
Samples every 10 m along the 5 m simplified track (the sampling of climb/band.py), matches each to the
nearest solved segment within 15 m and sums the rises of the solved heights at the foot points.
Usage: python3 -I solved_along.py GPX_DIR LINES.jsonl OUT.json [CACHE_DIR]
LINES.jsonl comes from dumplines.mjs; a ride counts when all its points lie in the box of the dump.
CACHE_DIR holds the swisstopo heights of band.py (<ride>-c.json) at the same samples."""
import sys, re, json, math, glob, os, bisect
from collections import defaultdict

DIR, LINES, OUT = sys.argv[1:4]
CACHE = sys.argv[4] if len(sys.argv) > 4 else '/media/timo/OSM1/tmp/rebake/climb/cache'
KINDS = {'main-road', 'road', 'path', 'tunnel', 'gallery'}
REACH, PREFER, CELL, STEP = 15.0, 3.0, 25.0, 1.0


def lv95(lat, lon):
    p = (lat * 3600 - 169028.66) / 10000; l = (lon * 3600 - 26782.5) / 10000
    return (2600072.37 + 211455.93 * l - 10938.51 * l * p - 0.36 * l * p * p - 44.54 * l ** 3,
            1200147.07 + 308807.95 * p + 3745.25 * l * l + 76.63 * p * p - 194.56 * l * l * p + 119.79 * p ** 3)


def parse(p):
    t = open(p, encoding='utf-8').read()
    pts = [(float(a), float(b), float(c)) for a, b, c in re.findall(r'<trkpt lat="([^"]+)" lon="([^"]+)">\s*<ele>([^<]+)</ele>', t)]
    return pts, re.findall(r'<type>([^<]+)</type>', t)[-1]


def simplify(xy, tol):
    keep = [False] * len(xy); keep[0] = keep[-1] = True; stack = [(0, len(xy) - 1)]
    while stack:
        i, j = stack.pop()
        if j <= i + 1: continue
        ax, ay = xy[i]; bx, by = xy[j]; dx, dy = bx - ax, by - ay; L = dx * dx + dy * dy
        best, bi = -1, -1
        for m in range(i + 1, j):
            px, py = xy[m]; t = 0 if L == 0 else max(0, min(1, ((px - ax) * dx + (py - ay) * dy) / L))
            d = math.hypot(px - ax - t * dx, py - ay - t * dy)
            if d > best: best, bi = d, m
        if best > tol: keep[bi] = True; stack += [(i, bi), (bi, j)]
    return [xy[i] for i, k in enumerate(keep) if k]


def samples_of(line):
    """Every 10 m along the line, carried over the vertices: the samples of band.py."""
    out = []; carry = 0.0
    for (ax, ay), (bx, by) in zip(line, line[1:]):
        L = math.hypot(bx - ax, by - ay)
        if L == 0: continue
        ux, uy = (bx - ax) / L, (by - ay) / L; d = carry
        while d < L:
            out.append((ax + ux * d, ay + uy * d)); d += 10.0
        carry = d - L
    return out


def gain(z): return sum(max(0.0, b - a) for a, b in zip(z, z[1:]))


def smooth(s, z, window):
    h2 = window / 2.0
    if len(z) < 3 or h2 <= 0: return list(z)
    area = [0.0]
    for k in range(1, len(z)): area.append(area[-1] + (s[k] - s[k - 1]) * (z[k - 1] + z[k]) / 2)
    def integral(x):
        k = min(max(bisect.bisect_right(s, x) - 1, 0), len(s) - 2)
        seg = s[k + 1] - s[k]; t = 0 if seg == 0 else (x - s[k]) / seg
        return area[k] + (x - s[k]) * (2 * z[k] + (z[k + 1] - z[k]) * t) / 2
    out = []
    for si, zi in zip(s, z):
        h = min(h2, si - s[0], s[-1] - si)
        out.append(zi if h <= 0 else (integral(si + h) - integral(si - h)) / (2 * h))
    return out


def cell(x, y): return (int(math.floor(x / CELL)), int(math.floor(y / CELL)))


def direction(lat, lon, angle, at):
    """Unit vector in LV95 of a Web Mercator tangent (radians from east toward south)."""
    e, n = math.cos(angle), -math.sin(angle)
    q = lv95(lat + n / 111195.0, lon + e / (111195.0 * math.cos(math.radians(lat))))
    dx, dy = q[0] - at[0], q[1] - at[1]; L = math.hypot(dx, dy)
    return dx / L, dy / L


# rides and their samples
rides = []
for p in sorted(glob.glob(DIR + '/*.gpx')):
    pts, kind = parse(p)
    line = simplify([lv95(a, b) for a, b, _ in pts], 5.0)
    rides.append({'name': os.path.basename(p)[:21], 'kind': kind, 'pts': pts, 'samples': samples_of(line)})

# box of the dump, and the cells near the rides
box = [180, 90, -180, -90]
with open(LINES) as fh:
    for row in fh:
        for lon, lat, *_ in json.loads(row)['v'][::8]:
            box = [min(box[0], lon), min(box[1], lat), max(box[2], lon), max(box[3], lat)]
inside = [r for r in rides if all(box[0] <= lon <= box[2] and box[1] <= lat <= box[3] for lat, lon, _ in r['pts'])]
print('dump box %.4f,%.4f,%.4f,%.4f; rides inside: %s' % (*box, ', '.join(r['name'] for r in inside) or 'none'))
near = set()
for r in inside:
    for x, y in r['samples']:
        cx, cy = cell(x, y)
        near.update((cx + i, cy + j) for i in (-1, 0, 1) for j in (-1, 0, 1))

# the pieces of the solved curves near the rides, each about 1 m long
pieces = []            # (ax, ay, bx, by, z_curve_a, z_curve_b, z_lin_a, z_lin_b, feature)
grid = defaultdict(list)
features = []          # (tile, f, kind, bridge)
stats = {'segments': 0, 'own': 0, 'near': 0, 'nan_vertices': 0, 'vertices': 0, 'no_plan_segments': 0, 'curve_vs_chord_max_m': 0.0}
mid_tiles = defaultdict(set)
with open(LINES) as fh:
    for row in fh:
        part = json.loads(row)
        v = part['v']; stats['vertices'] += len(v); stats['nan_vertices'] += sum(1 for q in v if q[2] is None)
        if part['kind'] not in KINDS: continue
        bridge = part['tags'].get('bridge', 'no') != 'no'
        fid = None
        xy = [lv95(lat, lon) for lon, lat, *_ in v]
        for i, own in enumerate(part['own']):
            stats['segments'] += 1
            if not own: continue
            stats['own'] += 1
            (ax, ay), (bx, by) = xy[i], xy[i + 1]
            c = math.hypot(bx - ax, by - ay)
            steps = max(1, int(math.ceil(c / 10.0)))
            if not any(cell(ax + (bx - ax) * k / steps, ay + (by - ay) * k / steps) in near for k in range(steps + 1)): continue
            z0, z1 = v[i][2], v[i + 1][2]
            if z0 is None or z1 is None: continue
            stats['near'] += 1
            if fid is None:
                fid = len(features); features.append((part['tile'], part['f'], part['kind'], bridge, part['tags'].get('highway', '')))
            mid_tiles[(round((ax + bx) / 4) , round((ay + by) / 4), part['kind'])].add(part['tile'])
            g0, g1, a0, a1 = v[i][3], v[i + 1][3], v[i][4], v[i + 1][4]
            n = max(1, int(math.ceil(c / STEP)))
            if g0 is None or a0 is None:
                stats['no_plan_segments'] += 1
                t0 = t1 = ((bx - ax) / c, (by - ay) / c) if c else (1.0, 0.0); g0 = g1 = (z1 - z0) / c if c else 0.0
            else:
                t0 = direction(v[i][1], v[i][0], a0, xy[i]); t1 = direction(v[i + 1][1], v[i + 1][0], a1, xy[i + 1])
            prev = None
            for k in range(n + 1):
                u = k / n
                h00, h10, h01, h11 = 2 * u ** 3 - 3 * u ** 2 + 1, u ** 3 - 2 * u ** 2 + u, -2 * u ** 3 + 3 * u ** 2, u ** 3 - u ** 2
                px = h00 * ax + h10 * c * t0[0] + h01 * bx + h11 * c * t1[0]
                py = h00 * ay + h10 * c * t0[1] + h01 * by + h11 * c * t1[1]
                zc = h00 * z0 + h10 * c * g0 + h01 * z1 + h11 * c * g1
                zl = z0 + u * (z1 - z0)
                stats['curve_vs_chord_max_m'] = max(stats['curve_vs_chord_max_m'], abs(zc - zl))
                if prev:
                    pieces.append((prev[0], prev[1], px, py, prev[2], zc, prev[3], zl, fid))
                    for cc in {cell(prev[0], prev[1]), cell(px, py)}: grid[cc].append(len(pieces) - 1)
                prev = (px, py, zc, zl)
dup = sum(1 for t in mid_tiles.values() if len(t) > 1)
stats['owned_midpoints_in_two_tiles'] = dup
print('dump: %(vertices)d vertices, %(nan_vertices)d with no height; %(segments)d segments of the kinds, %(own)d owned, %(near)d near a ride; no alignment on %(no_plan_segments)d' % stats)
print('owned segment midpoints (2 m cells) held by two tiles: %d' % dup)


def match(x, y):
    """Best piece per feature within REACH: {feature: (distance, z_curve, z_lin)}."""
    best = {}
    cx, cy = cell(x, y)
    for i in (-1, 0, 1):
        for j in (-1, 0, 1):
            for k in grid.get((cx + i, cy + j), ()):
                ax, ay, bx, by, za, zb, la, lb, f = pieces[k]
                dx, dy = bx - ax, by - ay; L = dx * dx + dy * dy
                t = 0 if L == 0 else max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / L))
                d = math.hypot(x - ax - t * dx, y - ay - t * dy)
                if d <= REACH and (f not in best or d < best[f][0]):
                    best[f] = (d, za + t * (zb - za), la + t * (lb - la))
    return best


def fill(z):
    """Bridges each gap of None linearly; holds the first and last known height at the ends."""
    known = [i for i, v in enumerate(z) if v is not None]
    if not known: return None
    out = list(z)
    for i in range(known[0]): out[i] = z[known[0]]
    for i in range(known[-1] + 1, len(z)): out[i] = z[known[-1]]
    for a, b in zip(known, known[1:]):
        for i in range(a + 1, b): out[i] = z[a] + (z[b] - z[a]) * (i - a) / (b - a)
    return out


def q(vals, f): v = sorted(vals); return v[min(len(v) - 1, int(f * len(v)))]


results = []
for r in inside:
    S = r['samples']; n = len(S); s = [10.0 * i for i in range(n)]
    zc, zl, kinds, classes, dists = [None] * n, [None] * n, [None] * n, [None] * n, []
    prev = None
    for i, (x, y) in enumerate(S):
        best = match(x, y)
        if not best: prev = None; continue
        f = min(best, key=lambda k: best[k][0])
        if prev in best and best[prev][0] <= best[f][0] + PREFER: f = prev
        d, zc[i], zl[i] = best[f]; dists.append(d); prev = f
        _, _, kind, bridge, hw = features[f]
        kinds[i] = 'bridge' if bridge else kind; classes[i] = hw
    gaps, run, start = [], 0, 0
    for i, k in enumerate(kinds + ['end']):
        if k is None:
            if run == 0: start = i
            run += 1
        elif run:
            gaps.append((round(s[start] / 1000, 2), run * 10)); run = 0
    share = {k: sum(1 for v in kinds if v == k) / n for k in sorted(set(kinds) - {None})}
    matched = sum(1 for v in kinds if v is not None) / n
    zcf, zlf = fill(zc), fill(zl)
    row = {'ride': r['name'], 'kind': r['kind'], 'samples': n, 'matched': matched, 'share_by_kind': share,
           'share_by_class': {k: round(sum(1 for v in classes if v == k) / n, 3) for k in sorted(set(classes) - {None})},
           'unmatched_m': sum(g[1] for g in gaps), 'longest_gaps_km_m': sorted(gaps, key=lambda g: -g[1])[:5],
           'match_distance_p50_m': q(dists, 0.5) if dists else None, 'match_distance_p90_m': q(dists, 0.9) if dists else None,
           'watch_up': gain([e for _, _, e in r['pts']])}
    if zcf:
        row['curve_minus_linear_max_m'] = max(abs(a - b) for a, b in zip(zcf, zlf))
        for w in (0, 25, 50, 100):
            row['solved_w%d' % w] = gain(smooth(s, zcf, w))
            row['solved_linear_w%d' % w] = gain(smooth(s, zlf, w))
        row['solved_net_m'] = zcf[-1] - zcf[0]; row['solved_range_m'] = max(zcf) - min(zcf)
    cf = os.path.join(CACHE, r['name'] + '-c.json')
    if zcf and os.path.exists(cf):
        terr = json.load(open(cf))
        row['cache_samples'] = len(terr)
        if len(terr) == n and all(t is not None for t in terr):
            for w in (0, 25, 50, 100): row['terrain_same_samples_w%d' % w] = gain(smooth(s, terr, w))
            diff = [zc[i] - terr[i] for i in range(n) if zc[i] is not None]
            m = sum(diff) / len(diff)
            row['solved_minus_terrain'] = {'mean': m, 'sd': math.sqrt(sum((d - m) ** 2 for d in diff) / len(diff)),
                                           'p05': q(diff, 0.05), 'p50': q(diff, 0.5), 'p95': q(diff, 0.95),
                                           'abs_p90': q([abs(d) for d in diff], 0.9), 'abs_max': max(abs(d) for d in diff), 'n': len(diff)}
    results.append(row)
    print(json.dumps(row))
json.dump({'lines': LINES, 'box': box, 'dump': stats, 'rides': results}, open(OUT, 'w'), indent=1)
