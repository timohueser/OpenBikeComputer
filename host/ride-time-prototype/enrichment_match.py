"""Offline geometry-only matching for a surface-data feasibility audit.

Costs and acceptance margins are heuristics, not calibrated match probabilities.
The graph is undirected: this reconstructs observed travel, not legal routing.
"""

from collections import defaultdict
from functools import lru_cache
import heapq
import math

import numpy as np
from shapely import LineString, Point, STRtree

TAGS = ("highway", "surface", "smoothness", "tracktype", "mtb:scale", "mtb:scale:uphill")
SETTINGS = dict(search_radius_m=40, max_candidates=8, gps_sigma_m=12, transition_scale_m=25,
                gap_seconds=30, network_limit_m=1200, accepted_offset_m=20, accepted_margin=3,
                accepted_length_ratio=[0.5, 1.5], accepted_length_difference_m=60)


def project(lonlat, origin):
    scale = np.array([math.cos(math.radians(origin[1])), 1.0]) * 111195.08
    return (np.asarray(lonlat) - origin) * scale


def decode_chain(emissions, transitions):
    """Minimum-cost sequence plus per-point margin against a different candidate."""
    forward, parents = [emissions[0].copy()], []
    for emission, transition in zip(emissions[1:], transitions):
        cost = forward[-1][:, None] + transition
        parent = np.argmin(cost, axis=0)
        parents.append(parent)
        forward.append(emission + cost[parent, np.arange(len(emission))])
    path = [int(np.argmin(forward[-1]))]
    for parent in reversed(parents):
        path.append(int(parent[path[-1]]))
    path.reverse()
    backward = [None] * len(emissions)
    backward[-1] = np.zeros_like(emissions[-1])
    for i in range(len(emissions)-2, -1, -1):
        backward[i] = np.min(transitions[i] + emissions[i+1][None, :] + backward[i+1][None, :], axis=1)
    totals = [a+b for a,b in zip(forward, backward)]
    return path, totals


class Network:
    def __init__(self, ways, origin):
        self.ways, self.origin = ways, origin
        self.adj = defaultdict(list)
        self.edges, self.lines = [], []
        for wi, way in enumerate(ways):
            xy = project(way["coordinates"], origin)
            for i, (a,b) in enumerate(zip(xy[:-1], xy[1:])):
                length = float(np.linalg.norm(b-a))
                if length < 0.01:
                    continue
                u, v = way["nodes"][i:i+2]
                edge = len(self.edges)
                self.edges.append((u,v,length,wi,a,b))
                self.lines.append(LineString([a,b]))
                self.adj[u].append((v,length,edge))
                self.adj[v].append((u,length,edge))
        self.tree = STRtree(self.lines)

    def candidates(self, xy):
        point = Point(xy)
        indices = self.tree.query(point, predicate="dwithin", distance=SETTINGS["search_radius_m"])
        candidates = []
        for edge in indices:
            u,v,length,way,a,b = self.edges[edge]
            fraction = float(np.clip(np.dot(xy-a,b-a)/length**2, 0, 1))
            offset = float(np.linalg.norm(xy-(a+fraction*(b-a))))
            candidates.append(dict(edge=int(edge), fraction=fraction, offset=offset, way=way))
        candidates.sort(key=lambda c:(c["offset"], c["edge"]))
        # Two nearby pieces of one winding way may both be plausible.
        counts, retained = defaultdict(int), []
        for c in candidates:
            if counts[c["way"]] < 2:
                retained.append(c)
                counts[c["way"]] += 1
            if len(retained) == SETTINGS["max_candidates"]:
                break
        return retained

    @lru_cache(maxsize=2048)
    def reachable(self, source):
        dist, previous, queue = {source:0.0}, {}, [(0.0,source)]
        while queue:
            distance,u = heapq.heappop(queue)
            if distance != dist[u]:
                continue
            for v,length,edge in self.adj[u]:
                candidate = distance+length
                if candidate <= SETTINGS["network_limit_m"] and candidate < dist.get(v, math.inf):
                    dist[v] = candidate
                    previous[v] = (u,edge)
                    heapq.heappush(queue,(candidate,v))
        return dist,previous

    def route(self, a, b, details=False):
        au,av,al,aw,*_ = self.edges[a["edge"]]
        bu,bv,bl,bw,*_ = self.edges[b["edge"]]
        best = abs(a["fraction"]-b["fraction"])*al if a["edge"]==b["edge"] else math.inf
        choice = None
        for source,first in ((au,a["fraction"]*al),(av,(1-a["fraction"])*al)):
            distances,_ = self.reachable(source)
            for target,last in ((bu,b["fraction"]*bl),(bv,(1-b["fraction"])*bl)):
                length = first+distances.get(target,math.inf)+last
                if length < best:
                    best,choice = length,(source,target,first,last)
        if not details:
            return best
        if not math.isfinite(best):
            return best,[]
        if choice is None:
            return best,[(aw,best)]
        source,target,first,last = choice
        _,previous = self.reachable(source)
        pieces = [(bw,last)]
        while target != source:
            target,edge = previous[target]
            pieces.append((self.edges[edge][3],self.edges[edge][2]))
        pieces.append((aw,first))
        return best,list(reversed(pieces))

    def match(self, track):
        xy = project(track[:,:2], self.origin)
        candidates = [self.candidates(p) for p in xy]
        chosen = np.full(len(track),-1,dtype=int)
        margin = np.zeros(len(track))
        connected = np.zeros(len(track)-1,dtype=bool)
        start = 0
        while start < len(track):
            if not candidates[start]:
                start += 1
                continue
            end,transitions = start+1,[]
            while end < len(track) and candidates[end]:
                dt = track[end,2]-track[end-1,2]
                chord = np.linalg.norm(xy[end]-xy[end-1])
                if not 0 < dt <= SETTINGS["gap_seconds"] or chord > 750:
                    break
                distances = np.array([[self.route(a,b) for b in candidates[end]] for a in candidates[end-1]])
                transition = np.abs(distances-chord)/SETTINGS["transition_scale_m"]
                # A disconnected chain cannot support an inferred transition.
                if not np.isfinite(transition).any():
                    break
                transitions.append(transition)
                end += 1
            emissions = [np.array([c["offset"]**2/(2*SETTINGS["gps_sigma_m"]**2) for c in row])
                         for row in candidates[start:end]]
            path, totals = decode_chain(emissions,transitions)
            # Pairwise connections need not form a complete path through a chain.
            if not np.isfinite(totals[-1]).any():
                end = start+1
                path,totals = decode_chain(emissions[:1],[])
            for local,(index,costs) in enumerate(zip(path,totals)):
                i = start+local
                chosen[i] = index
                alternative = [cost for c,cost in zip(candidates[i],costs)
                               if c["way"] != candidates[i][index]["way"]]
                margin[i] = min(alternative,default=math.inf)-costs[index]
            connected[start:end-1] = True
            start = end
        return xy,candidates,chosen,margin,connected


def interval_status(a, b, margin_a, margin_b, connected, valid, chord, length):
    if not valid:
        return "timing_or_geometry_screen"
    if a is None or b is None:
        return "no_candidate"
    if not connected or not math.isfinite(length):
        return "disconnected_or_gap"
    if max(a["offset"],b["offset"]) > SETTINGS["accepted_offset_m"]:
        return "large_offset"
    if min(margin_a,margin_b) < SETTINGS["accepted_margin"]:
        return "ambiguous"
    lo,hi = SETTINGS["accepted_length_ratio"]
    if not lo*chord <= length <= hi*chord or abs(length-chord) > SETTINGS["accepted_length_difference_m"]:
        return "length_disagreement"
    return "accepted"
