"""Prototype, never merges: the map tools of a route reconstruction agent, and the scoring of its plans.

    python3 maptool.py find OSM.json "NAME" [LAT,LON]
    python3 maptool.py around OSM.json LAT,LON RADIUS_M
    python3 maptool.py relation OSM.json RELATION_ID
    python3 maptool.py route OSM.json "LAT,LON;LAT,LON;..."
    python3 maptool.py score OSM.json ROUTES.json PLANS.json OUTDIR

A plan is the ordered points of a route: the router joins them on the OSM ways and prefers the
ways of signed hiking routes. `score` compares each planned line with the stated length and with
the truth that the agent never saw: the office's own GPX, or else the OSM relation of the same name.
"""
import json
import math
import os
import pickle
import re
import sys
from collections import defaultdict

from reconstruct import Area, NEAR_M, away_share, densify, gpx_line, norm, similarity, variants

PENALTY = 2.0


def load(path):
    cache = path + ".pkl"
    if os.path.exists(cache) and os.path.getmtime(cache) > os.path.getmtime(path):
        return pickle.load(open(cache, "rb"))
    area = Area(path)
    area.adj = dict(area.adj)
    area.way_relations = defaultdict(list)
    for i, (_, ways) in enumerate(area.relations):
        for w in ways:
            area.way_relations[w].append(i)
    area.way_relations = dict(area.way_relations)
    area.edge_way = {}
    for w, nodes in area.way_nodes.items():
        for a, b in zip(nodes, nodes[1:]):
            area.edge_way[(a, b)] = area.edge_way[(b, a)] = w
    area.way_name = {int(f[4][1:]): f[0] for f in area.features if f[1] == "street"}
    pickle.dump(area, open(cache, "wb"))
    return area


def point(text):
    lat, lon = text.split(",")
    return float(lat), float(lon)


def label(tags):
    parts = [tags.get("name") or "(no name)"]
    parts += [f"{k}={tags[k]}" for k in ("ref", "osmc:symbol", "network", "roundtrip", "distance") if k in tags]
    return ", ".join(parts)


def chain_points(area, ways, spacing=250.0):
    """The ways of a relation joined greedily at their nearest ends, sampled each `spacing` metres."""
    lines = [[area.coord[n] for n in area.way_nodes[w]] for w in ways if w in area.way_nodes]
    if not lines:
        return [], 0.0
    line = lines.pop(0)
    while lines:
        k, flip = min(((k, f) for k in range(len(lines)) for f in (False, True)),
                      key=lambda kf: area.dist(line[-1], lines[kf[0]][-1 if kf[1] else 0]))
        nxt = lines.pop(k)
        line += nxt[::-1] if flip else nxt
    out, run, total = [line[0]], 0.0, 0.0
    for a, b in zip(line, line[1:]):
        d = area.dist(a, b)
        total += d if d < 500 else 0.0
        run += d
        if run >= spacing:
            out.append(b)
            run = 0.0
    return out + [line[-1]], total


def cmd_find(area, name, near=None):
    vs = variants(name)
    hits = []
    for f, tokens in zip(area.features, area.feature_tokens):
        score = max((similarity(v, tokens) for v in vs), default=0.0)
        if score >= 0.7:
            at = (f[2], f[3])
            if f[1] == "street" and near:
                at = min((area.coord[n] for n in area.way_nodes[int(f[4][1:])]), key=lambda c: area.dist(c, near))
            hits.append((score, area.dist(at, near) if near else 0.0, f, at))
    hits.sort(key=lambda h: (-round(h[0], 1), h[1]))
    for score, d, f, at in hits[:25]:
        far = f"  {d:.0f} m away" if near else ""
        print(f"{score:.2f}  {f[0]}  [{f[1]}]  {at[0]:.5f},{at[1]:.5f}{far}")
    for i, (tags, ways) in enumerate(area.relations):
        score = max((similarity(v, norm(tags.get("name", ""))) for v in vs), default=0.0)
        if score >= 0.7:
            print(f"{score:.2f}  hiking route relation {i}: {label(tags)}")
    if not hits:
        print("no named feature matches")


def cmd_around(area, at, radius):
    rows = sorted((area.dist(at, (f[2], f[3])), f) for f in area.features if f[1] != "street")
    for d, f in rows:
        if d <= radius:
            print(f"{d:5.0f} m  {f[0]}  [{f[1]}]  {f[2]:.5f},{f[3]:.5f}")
    ways, streets = set(), {}
    for k in area.tree.query_ball_point(area.xy(at), radius):
        n = area.node_ids[k]
        for nbr, *_ in area.adj.get(n, []):
            w = area.edge_way[(n, nbr)]
            ways.add(w)
            if w in area.way_name:
                streets[area.way_name[w]] = area.coord[n]
    for name, c in sorted(streets.items()):
        print(f"street/path name: {name}  near {c[0]:.5f},{c[1]:.5f}")
    for i in sorted({i for w in ways for i in area.way_relations.get(w, [])}):
        print(f"hiking route relation {i}: {label(area.relations[i][0])}")


def cmd_relation(area, i):
    tags, ways = area.relations[i]
    pts, total = chain_points(area, ways)
    print(label(tags))
    print(f"about {total / 1000:.1f} km of ways in this area; ends {area.dist(pts[0], pts[-1]):.0f} m apart" if pts else "no ways in this area")
    print("points along it, each about 250 m: " + ";".join(f"{p[0]:.5f},{p[1]:.5f}" for p in pts))


def run_route(area, pts):
    stops = [area.snap(*p) for p in pts]
    nodes, legs = [], []
    for a, b in zip(stops, stops[1:]):
        leg = area.path(a, b, PENALTY) if a != b else [a]
        if leg is None:
            return None, legs
        legs.append(area.hiking_share(leg)[0])
        nodes += leg if not nodes else leg[1:]
    return nodes, legs


def cmd_route(area, text):
    pts = [point(p) for p in text.split(";")]
    nodes, legs = run_route(area, pts)
    if nodes is None:
        print(f"no way between point {len(legs) + 1} and point {len(legs) + 2}")
        return
    total, share = area.hiking_share(nodes)
    print(f"length {total / 1000:.2f} km, {share:.0%} on signed hiking ways, ends {area.dist(area.coord[nodes[0]], area.coord[nodes[-1]]):.0f} m apart")
    print("legs (m): " + ", ".join(f"{l:.0f}" for l in legs))
    snaps = [area.dist(p, area.coord[area.snap(*p)]) for p in pts]
    if max(snaps) > 60:
        print("points far from any way (m): " + ", ".join(f"{i + 1}: {s:.0f}" for i, s in enumerate(snaps) if s > 60))
    print("in order along the line:")
    at, last, seen = 0.0, None, set()
    names = [(area.tree.query(area.xy((f[2], f[3]))), f) for f in area.features if f[1] != "street"]
    by_node = defaultdict(list)
    for (d, k), f in names:
        if d <= 40:
            by_node[area.node_ids[k]].append(f[0] + f" [{f[1]}]")
    for a, b in zip(nodes, nodes[1:]):
        w = area.edge_way[(a, b)]
        rels = "; ".join(label(area.relations[i][0]) for i in area.way_relations.get(w, [])[:3])
        here = (area.way_name.get(w, ""), rels)
        if here != last:
            print(f"  {at / 1000:5.2f} km  way: {here[0] or '(no name)'}  | signed routes: {here[1] or 'none'}")
            last = here
        for n in by_node.get(b, []):
            if n not in seen:
                seen.add(n)
                print(f"  {at / 1000:5.2f} km  passes {n}")
        at += area.dist(area.coord[a], area.coord[b])


def cmd_score(area, routes_path, plans_path, out):
    routes = {r["name"]: r for r in json.load(open(routes_path))["routes"]}
    rows = []
    for plan in json.load(open(plans_path)):
        r = routes[plan["route"]]
        row = {"route": plan["route"], "points": len(plan["points"]), "stated_km": r.get("length_km"),
               "confidence": plan.get("confidence")}
        nodes, _ = run_route(area, [tuple(p) for p in plan["points"]]) if len(plan["points"]) > 1 else (None, [])
        if nodes and len(nodes) > 1:
            total, share = area.hiking_share(nodes)
            line = [area.coord[n] for n in nodes]
            row.update(km=round(total / 1000, 2), on_hiking=round(share, 2))
            if row["stated_km"]:
                row["km_ratio"] = round(row["km"] / row["stated_km"], 2)
            local = (r.get("geometry") or {}).get("local_file")
            truth, source = (gpx_line(local), "gpx") if local and local.endswith(".gpx") else (None, None)
            if truth is None:
                truth, source = area.relation_line(plan["route"]), "osm relation"
                if truth and min(area.dist(line[0], c) for seg in truth for c in seg) > 1500:
                    truth = None  # a route of the same name in another village
            if truth:
                mine, theirs = densify([line], area), densify(truth, area)
                row.update(truth=source, truth_km=round(len(theirs) * 5 / 1000, 2),
                           mine_off_truth=round(away_share(mine, theirs), 2),
                           truth_missed=round(away_share(theirs, mine), 2))
            slug = re.sub(r"[^a-z0-9]+", "-", " ".join(norm(plan["route"])))[:50]
            feats = [{"type": "Feature", "properties": {"role": "reconstruction", **row},
                      "geometry": {"type": "LineString", "coordinates": [[c[1], c[0]] for c in line]}}]
            feats += [{"type": "Feature", "properties": {"role": "truth", "source": source},
                       "geometry": {"type": "LineString", "coordinates": [[c[1], c[0]] for c in seg]}}
                      for seg in (truth or [])]
            feats += [{"type": "Feature", "properties": {"role": "plan point", "index": i},
                       "geometry": {"type": "Point", "coordinates": [p[1], p[0]]}} for i, p in enumerate(plan["points"])]
            json.dump({"type": "FeatureCollection", "features": feats}, open(f"{out}/{slug}.geojson", "w"), ensure_ascii=False)
        rows.append(row)
    json.dump(rows, open(f"{out}/summary.json", "w"), ensure_ascii=False, indent=1)
    keys = ["route", "points", "stated_km", "km", "km_ratio", "on_hiking", "truth", "truth_km", "mine_off_truth",
            "truth_missed", "confidence"]
    print(" | ".join(keys))
    for row in rows:
        print(" | ".join(str(row.get(k, "")) for k in keys))


def main():
    cmd, osm, *args = sys.argv[1:]
    area = load(osm)
    if cmd == "find":
        cmd_find(area, args[0], point(args[1]) if len(args) > 1 else None)
    elif cmd == "around":
        cmd_around(area, point(args[0]), float(args[1]))
    elif cmd == "relation":
        cmd_relation(area, int(args[0]))
    elif cmd == "route":
        cmd_route(area, args[0])
    elif cmd == "score":
        cmd_score(area, *args)


if __name__ == "__main__":
    main()
