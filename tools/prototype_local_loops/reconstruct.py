"""Prototype, never merges: rebuilds a hiking loop from the place names of a written description.

    python3 reconstruct.py candidates OSM.json ROUTES.json OUT.json
    python3 reconstruct.py route OSM.json ROUTES.json OUTDIR [--choices FILE] [--penalty 2.0]

OSM.json is an Overpass answer with the walkable ways (with geometry), the hiking relations and
the named features of the area. ROUTES.json is the output of an indexing agent (see BRIEF.md).
`candidates` lists, for each waypoint of each route, the named OSM features that could be it.
`route` picks one candidate for each waypoint (from --choices, else the chain with the shortest
straight-line length), routes between them on the OSM ways, and compares the line with the truth:
the stated length, and the office's own GPX or the OSM relation of the same name when one exists.
"""
import difflib
import heapq
import json
import math
import re
import sys
import unicodedata
import xml.etree.ElementTree as ET
from collections import defaultdict

import numpy as np
from scipy.spatial import cKDTree

STOP = set("de du la le les des l d der die das den dem am im zum zur beim an in von vom und et a au aux "
           "sur vers en un une the of".split())
FOOT = {"path", "footway", "track", "steps", "pedestrian", "residential", "service", "unclassified",
        "living_street", "cycleway", "bridleway", "road"}
ROAD_FACTOR = {"tertiary": 2.0, "tertiary_link": 2.0, "secondary": 4.0, "secondary_link": 4.0,
               "primary": 4.0, "primary_link": 4.0}
MATCH_MIN = 0.8
MAX_CANDIDATES = 80
NEAR_M = 30.0  # the deviation distance of specs/route-catalog.md


def norm(text):
    text = unicodedata.normalize("NFKD", text.lower().replace("ß", "ss"))
    text = "".join(c for c in text if not unicodedata.combining(c))
    return [t for t in re.split(r"[^a-z0-9]+", text) if t and t not in STOP]


def similarity(a, b):
    if not a or not b:
        return 0.0
    ratio = difflib.SequenceMatcher(None, " ".join(a), " ".join(b)).ratio()
    common = len(set(a) & set(b))
    contained = (common / len(a)) * (0.85 + 0.15 * common / len(b)) if common == len(set(a)) else 0.0
    return max(ratio, contained)


def variants(name):
    out = [name, re.sub(r"\([^)]*\)", " ", name)]
    out += re.findall(r"\(([^)]*)\)", name)
    out += [part for part in re.split(r"\s*/\s*", name) if part != name]
    return [v for v in (norm(v) for v in out) if v]


def metres(lat):
    return 111_320.0, 111_320.0 * math.cos(math.radians(lat))


class Area:
    def __init__(self, path):
        elements = json.load(open(path))["elements"]
        hiking = set()
        self.relations = []
        for e in elements:
            if e["type"] == "relation":
                ways = [m["ref"] for m in e.get("members", []) if m["type"] == "way"]
                hiking.update(ways)
                self.relations.append((e.get("tags", {}), ways))
        self.coord = {}
        self.adj = defaultdict(list)
        self.way_nodes = {}
        self.features = []
        for e in elements:
            tags = e.get("tags", {})
            if e["type"] == "way" and "highway" in tags and "geometry" in e:
                nodes = e["nodes"]
                self.way_nodes[e["id"]] = nodes
                for n, g in zip(nodes, e["geometry"]):
                    self.coord[n] = (g["lat"], g["lon"])
                on_hiking = e["id"] in hiking
                kind = tags["highway"]
                factor = ROAD_FACTOR.get(kind, 1.0 if kind in FOOT else 3.0)
                if tags.get("foot") in ("no", "private") or tags.get("access") in ("no", "private"):
                    factor = 50.0
                for a, b in zip(nodes, nodes[1:]):
                    length = self.dist(self.coord[a], self.coord[b])
                    self.adj[a].append((b, length, on_hiking, factor))
                    self.adj[b].append((a, length, on_hiking, factor))
                if tags.get("name"):
                    lat, lon = self.coord[nodes[len(nodes) // 2]]
                    self.features.append((tags["name"], "street", lat, lon, f"w{e['id']}"))
            elif "name" in tags and e["type"] in ("node", "way"):
                at = e if e["type"] == "node" else e.get("center")
                if at:
                    kind = next((f"{k}={tags[k]}" for k in ("information", "tourism", "amenity", "historic",
                                 "natural", "place", "man_made", "waterway", "building", "shop", "leisure",
                                 "railway", "highway") if k in tags), "named")
                    self.features.append((tags["name"], kind, at["lat"], at["lon"], f"{e['type'][0]}{e['id']}"))
        self.feature_tokens = [norm(f[0]) for f in self.features]
        self.node_ids = list(self.coord)
        lat0 = sum(c[0] for c in self.coord.values()) / len(self.coord)
        self.my, self.mx = metres(lat0)
        self.tree = cKDTree([self.xy(self.coord[n]) for n in self.node_ids])

    def xy(self, c):
        return (c[1] * self.mx, c[0] * self.my)

    def dist(self, a, b):
        my, mx = metres(a[0])
        return math.hypot((a[0] - b[0]) * my, (a[1] - b[1]) * mx)

    def candidates(self, name):
        vs = variants(name)
        scored = []
        for feature, tokens in zip(self.features, self.feature_tokens):
            score = max((similarity(v, tokens) for v in vs), default=0.0)
            if score >= MATCH_MIN:
                scored.append((score, feature))
        scored.sort(key=lambda s: -s[0])
        return [{"id": f[4], "name": f[0], "kind": f[1], "lat": f[2], "lon": f[3], "score": round(s, 2)}
                for s, f in scored[:MAX_CANDIDATES]]

    def snap(self, lat, lon):
        return self.node_ids[self.tree.query(self.xy((lat, lon)))[1]]

    def path(self, start, goal, penalty):
        best = {start: 0.0}
        prev = {}
        heap = [(0.0, start)]
        while heap:
            cost, node = heapq.heappop(heap)
            if node == goal:
                break
            if cost > best[node]:
                continue
            for nbr, length, on_hiking, factor in self.adj[node]:
                step = length * (1.0 if on_hiking else penalty * factor)
                if cost + step < best.get(nbr, math.inf):
                    best[nbr] = cost + step
                    prev[nbr] = node
                    heapq.heappush(heap, (cost + step, nbr))
        if goal not in best:
            return None
        nodes = [goal]
        while nodes[-1] != start:
            nodes.append(prev[nodes[-1]])
        return nodes[::-1]

    def hiking_share(self, nodes):
        total = on = 0.0
        for a, b in zip(nodes, nodes[1:]):
            edge = next(e for e in self.adj[a] if e[0] == b)
            total += edge[1]
            on += edge[1] if edge[2] else 0.0
        return total, (on / total if total else 0.0)

    def relation_line(self, name):
        """The ways of the hiking relation whose name is nearest to `name`, as loose segments."""
        want = norm(name)
        score, ways = max(((max(similarity(want, norm(t.get("name", ""))), similarity(want[:1], norm(t.get("name", "")))), w)
                           for t, w in self.relations), key=lambda s: s[0], default=(0, []))
        if score < 0.7:
            return None
        return [[self.coord[n] for n in self.way_nodes[w]] for w in ways if w in self.way_nodes]


def cluster(route, place, cands, area):
    """Keeps the candidates of the one disc that explains the most waypoints.

    The disc has the radius of half the stated length. A name that is rare counts more than a
    common one, and a disc with the village of the route in it gets a bonus."""
    radius = max(600.0, 500.0 * route["length_km"]) if route.get("length_km") else 4000.0
    text = set(norm(" ".join(filter(None, [route["name"], route.get("start"), place]))))
    villages = [(f[2], f[3]) for f, t in zip(area.features, area.feature_tokens)
                if f[1].startswith("place=") and t and set(t) <= text]
    best = None
    for centre in (c for cs in cands for c in cs):
        at = (centre["lat"], centre["lon"])
        score = 1.0 if any(area.dist(at, v) < max(radius, 2000.0) for v in villages) else 0.0
        for cs in cands:
            near = [c["score"] for c in cs if area.dist(at, (c["lat"], c["lon"])) <= radius]
            score += max(near) / math.sqrt(len(cs)) if near else 0.0
        if best is None or score > best[0]:
            best = (score, at)
    if best is None:
        return cands
    return [[c for c in cs if area.dist(best[1], (c["lat"], c["lon"])) <= radius] for cs in cands]


def chain(cands, area, loop):
    """One candidate for each waypoint that has one: the chain with the shortest straight-line length."""
    idx = [i for i, c in enumerate(cands) if c]
    if not idx:
        return {}
    def step(a, b):
        return area.dist((a["lat"], a["lon"]), (b["lat"], b["lon"])) + 400.0 * (1.0 - b["score"])
    best = None
    for first in cands[idx[0]]:  # fix the start, so that a loop can close on it
        cost = {id(first): 0.0}
        back = {}
        layer = [first]
        for i in idx[1:]:
            for c in cands[i]:
                p = min(layer, key=lambda p: cost[id(p)] + step(p, c))
                cost[id(c)] = cost[id(p)] + step(p, c)
                back[id(c)] = p
            layer = cands[i]
        last = min(layer, key=lambda c: cost[id(c)] + (step(c, first) if loop else 0.0))
        total = cost[id(last)] + (step(last, first) if loop else 0.0)
        if best is None or total < best[0]:
            picks = [last]
            while id(picks[-1]) in back:
                picks.append(back[id(picks[-1])])
            best = (total, picks[::-1])
    return dict(zip(idx, best[1]))


def densify(lines, area, spacing=5.0):
    pts = []
    for line in lines:
        for a, b in zip(line, line[1:]):
            ax, ay = area.xy(a)
            bx, by = area.xy(b)
            n = max(1, int(math.hypot(bx - ax, by - ay) / spacing))
            pts += [(ax + (bx - ax) * k / n, ay + (by - ay) * k / n) for k in range(n)]
    return np.array(pts)


def away_share(a, b):
    """Share of the points of `a` that are more than NEAR_M from `b`."""
    return float((cKDTree(b).query(a)[0] > NEAR_M).mean())


def gpx_line(path):
    root = ET.parse(path).getroot()
    pts = [(float(e.get("lat")), float(e.get("lon"))) for e in root.iter() if e.tag.endswith(("trkpt", "rtept"))]
    return [pts] if len(pts) > 1 else None


def main():
    mode, osm, routes_path, out = sys.argv[1:5]
    opts = dict(zip(sys.argv[5::2], sys.argv[6::2]))
    area = Area(osm)
    index = json.load(open(routes_path))
    routes = index["routes"]
    all_cands = [cluster(r, index.get("place"), [area.candidates(w["name"]) for w in r.get("waypoints") or []], area)
                 for r in routes]
    if mode == "candidates":
        json.dump([{"route": r["name"], "place": index.get("place"), "stated_km": r.get("length_km"), "start": r.get("start"), "description_text": r.get("description_text"),
                    "waypoints": [{"index": i, "name": w["name"], "cue": w.get("cue"), "candidates": c}
                                  for i, (w, c) in enumerate(zip(r["waypoints"], cs))]}
                   for r, cs in zip(routes, all_cands) if len(r.get("waypoints") or []) > 2],
                  open(out, "w"), ensure_ascii=False, indent=1)
        return
    penalty = float(opts.get("--penalty", 2.0))
    choices = {c["route"]: c["choices"] for c in json.load(open(opts["--choices"]))} if "--choices" in opts else None
    rows = []
    for r, cands in zip(routes, all_cands):
        if len(cands) <= 2:
            continue
        loop = r.get("loop") is not False
        if choices is not None:
            by_id = [{c["id"]: c for c in cs} for cs in cands]
            picks = {int(i): by_id[int(i)][cid] for i, cid in (choices.get(r["name"]) or {}).items()
                     if cid and cid in by_id[int(i)]}
        else:
            picks = chain(cands, area, loop)
        row = {"route": r["name"], "waypoints": len(cands), "matched": len(picks), "stated_km": r.get("length_km")}
        stops = [area.snap(picks[i]["lat"], picks[i]["lon"]) for i in sorted(picks)]
        stops = [s for k, s in enumerate(stops) if k == 0 or s != stops[k - 1]]
        if loop and len(stops) > 1 and stops[-1] != stops[0]:
            stops.append(stops[0])
        nodes = []
        for a, b in zip(stops, stops[1:]):
            leg = area.path(a, b, penalty)
            if leg is None:
                nodes = []
                break
            nodes += leg if not nodes else leg[1:]
        if len(nodes) > 1:
            length, share = area.hiking_share(nodes)
            line = [area.coord[n] for n in nodes]
            row.update(km=round(length / 1000, 2), on_hiking=round(share, 2))
            if row["stated_km"]:
                row["km_ratio"] = round(length / 1000 / row["stated_km"], 2)
            local = (r.get("geometry") or {}).get("local_file")
            truth, source = (gpx_line(local), "gpx") if local and local.endswith(".gpx") else (None, None)
            if truth is None:
                truth, source = area.relation_line(r["name"]), "osm relation"
            if truth:
                mine, theirs = densify([line], area), densify(truth, area)
                row.update(truth=source, truth_km=round(len(theirs) * 5 / 1000, 2),
                           mine_off_truth=round(away_share(mine, theirs), 2),
                           truth_missed=round(away_share(theirs, mine), 2))
            slug = re.sub(r"[^a-z0-9]+", "-", " ".join(norm(r["name"])))[:50]
            json.dump({"type": "FeatureCollection", "features": [
                {"type": "Feature", "properties": {"name": r["name"], "role": "reconstruction", **row},
                 "geometry": {"type": "LineString", "coordinates": [[c[1], c[0]] for c in line]}},
                *({"type": "Feature", "properties": {"role": "truth"},
                   "geometry": {"type": "LineString", "coordinates": [[c[1], c[0]] for c in seg]}}
                  for seg in (truth or [])),
                *({"type": "Feature", "properties": {"role": "waypoint", "index": i, "name": r["waypoints"][i]["name"],
                                                     "osm": p["name"]},
                   "geometry": {"type": "Point", "coordinates": [p["lon"], p["lat"]]}} for i, p in picks.items()),
            ]}, open(f"{out}/{slug}.geojson", "w"), ensure_ascii=False)
        rows.append(row)
    json.dump(rows, open(f"{out}/summary.json", "w"), ensure_ascii=False, indent=1)
    keys = ["route", "waypoints", "matched", "stated_km", "km", "km_ratio", "on_hiking", "truth", "truth_km",
            "mine_off_truth", "truth_missed"]
    print(" | ".join(keys))
    for row in rows:
        print(" | ".join(str(row.get(k, "")) for k in keys))


if __name__ == "__main__":
    main()
