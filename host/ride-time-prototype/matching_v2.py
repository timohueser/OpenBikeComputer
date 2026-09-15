"""Compare complete candidate transitions, not isolated OSM way identifiers.

Agreement is conditional on retained candidates and one shortest path per pair.
The cost margin is not a probability or an exhaustive alternative-route search.
"""

from collections import Counter
import math

import numpy as np

from enrichment_match import Network, SETTINGS, TAGS

CONFIG = dict(SETTINGS, endpoint_tolerance_m=20.0, minimum_shared_fraction=0.5)


def solve_chain(emissions, transitions):
    forward, parents = [emissions[0].copy()], []
    for emission, transition in zip(emissions[1:], transitions):
        costs = forward[-1][:,None]+transition
        parent = np.argmin(costs,axis=0)
        parents.append(parent)
        forward.append(emission+costs[parent,np.arange(len(emission))])
    backward = [None]*len(emissions)
    backward[-1] = np.zeros_like(emissions[-1])
    for i in range(len(transitions)-1,-1,-1):
        backward[i] = np.min(transitions[i]+emissions[i+1][None,:]+backward[i+1][None,:],axis=1)
    path = [int(np.argmin(forward[-1]))]
    for parent in reversed(parents):
        path.append(int(parent[path[-1]]))
    path.reverse()
    best = float(np.min(forward[-1]))
    pairs = [forward[i][:,None]+transition+emissions[i+1][None,:]+backward[i+1][None,:]-best
             for i,transition in enumerate(transitions)]
    return path,pairs


def trim(pieces, metres):
    """Remove the same distance from each end; pieces are directed edge intervals."""
    result = [list(p) for p in pieces]
    for reverse in (False,True):
        remaining = metres
        order = range(len(result)-1,-1,-1) if reverse else range(len(result))
        for i in order:
            edge,a,b = result[i]
            amount = min(abs(b-a),remaining)
            direction = 1 if b>=a else -1
            result[i][2 if reverse else 1] += (-direction if reverse else direction)*amount
            remaining -= amount
            if remaining<=1e-9:
                break
    return [tuple(p) for p in result if abs(p[2]-p[1])>1e-9]


def overlap(a,b):
    total = 0.0
    for edge,lo,hi in a:
        lo,hi = sorted((lo,hi))
        intervals = sorted((max(lo,min(x,y)),min(hi,max(x,y))) for e,x,y in b if e==edge)
        end = lo
        for start,stop in intervals:
            total += max(0,stop-max(end,start))
            end = max(end,stop)
    return total


def same_core(a,b,tolerance=10):
    la,lb = sum(abs(y-x) for _,x,y in a),sum(abs(y-x) for _,x,y in b)
    if min(la,lb)<=0:
        return False
    if min(overlap(a,b),overlap(b,a)) < CONFIG["minimum_shared_fraction"]*max(la,lb)-1e-7:
        return False
    for route,other in ((a,b),(b,a)):
        core = trim(route,tolerance)
        length = sum(abs(y-x) for _,x,y in core)
        if overlap(core,other)<length-1e-7:
            return False
    return True


def common_composition(mixes):
    """Lower bound on each tag's fraction across all considered paths."""
    if not mixes:
        return {}
    values = set.intersection(*(set(mix) for mix in mixes))
    return {key:min(mix[key] for mix in mixes) for key in values if key!="unknown"}


class PathNetwork(Network):
    def __init__(self,ways,origin):
        super().__init__(ways,origin)
        self.corridors=np.full(len(self.edges),-1,dtype=np.int32)
        next_id=0
        # Collapse only unbranched chains. Closed rings keep distinct edge IDs.
        for node,adjacent in self.adj.items():
            if len(adjacent)==2:
                continue
            for neighbor,_,edge in adjacent:
                if self.corridors[edge]>=0:
                    continue
                self.corridors[edge]=next_id
                prior,current_edge,current=node,edge,neighbor
                while len(self.adj[current])==2:
                    options=[p for p in self.adj[current] if p[2]!=current_edge]
                    if not options:
                        break
                    following,_,following_edge=options[0]
                    if self.corridors[following_edge]>=0:
                        break
                    self.corridors[following_edge]=next_id
                    prior,current,current_edge=current,following,following_edge
                next_id+=1
        for edge in np.flatnonzero(self.corridors<0):
            self.corridors[edge]=next_id
            next_id+=1

    def same_path(self,a,b,tolerance=None):
        tolerance=CONFIG["endpoint_tolerance_m"] if tolerance is None else tolerance
        ca={int(self.corridors[e]) for e,x,y in a if abs(x-y)>1e-9}
        cb={int(self.corridors[e]) for e,x,y in b if abs(x-y)>1e-9}
        if len(ca)==1 and ca==cb:
            return True
        return same_core(a,b,tolerance)

    def candidates(self, xy):
        # Request the untruncated neighborhood so duplicate junction endpoints do
        # not use candidate slots. The graph node, not a way ID, defines equality.
        from shapely import Point
        indices = self.tree.query(Point(xy),predicate="dwithin",distance=CONFIG["search_radius_m"])
        rows = []
        for edge in indices:
            u,v,length,way,a,b = self.edges[edge]
            fraction = float(np.clip(np.dot(xy-a,b-a)/length**2,0,1))
            key = ("node",u) if fraction==0 else ("node",v) if fraction==1 else ("edge",int(edge))
            rows.append(dict(edge=int(edge),fraction=fraction,offset=float(np.linalg.norm(xy-(a+fraction*(b-a)))),way=way,key=key))
        rows.sort(key=lambda c:(c["offset"],c["edge"]))
        retained,seen,counts = [],set(),Counter()
        for c in rows:
            if c["key"] in seen or counts[c["way"]]>=2:
                continue
            seen.add(c["key"])
            counts[c["way"]]+=1
            retained.append(c)
            if len(retained)==CONFIG["max_candidates"]:
                break
        return retained

    def route_pieces(self,a,b):
        au,av,al,*_ = self.edges[a["edge"]]
        bu,bv,bl,*_ = self.edges[b["edge"]]
        pa,pb = a["fraction"]*al,b["fraction"]*bl
        best = abs(pa-pb) if a["edge"]==b["edge"] else math.inf
        choice = None
        for source,first,sa in ((au,pa,0.0),(av,al-pa,al)):
            distances,_ = self.reachable(source)
            for target,last,tb in ((bu,pb,0.0),(bv,bl-pb,bl)):
                length=first+distances.get(target,math.inf)+last
                if length<best:
                    best,choice=length,(source,target,sa,tb)
        if not math.isfinite(best):
            return best,[]
        if choice is None:
            return best,[(a["edge"],pa,pb)] if best else []
        source,target,sa,tb = choice
        _,previous = self.reachable(source)
        pieces = [(b["edge"],tb,pb)]
        while target!=source:
            prior,edge = previous[target]
            u,v,length,*_ = self.edges[edge]
            pieces.append((edge,0.0,length) if prior==u else (edge,length,0.0))
            target=prior
        pieces.append((a["edge"],pa,sa))
        return best,[p for p in reversed(pieces) if abs(p[2]-p[1])>1e-9]

    def analyse(self,track):
        from enrichment_match import project
        xy=project(track[:,:2],self.origin)
        candidates=[self.candidates(p) for p in xy]
        chosen=np.full(len(track),-1,dtype=int)
        alternatives=[[] for _ in range(len(track)-1)]
        start=0
        while start<len(track):
            if not candidates[start]:
                start+=1
                continue
            end,transitions=start+1,[]
            emissions=[np.array([c["offset"]**2/(2*CONFIG["gps_sigma_m"]**2) for c in candidates[start]])]
            reachable=emissions[0]
            while end<len(track) and candidates[end]:
                dt=track[end,2]-track[end-1,2]
                chord=float(np.linalg.norm(xy[end]-xy[end-1]))
                if not 0<dt<=CONFIG["gap_seconds"] or chord>750:
                    break
                distances=np.array([[self.route(a,b) for b in candidates[end]] for a in candidates[end-1]])
                transition=np.abs(distances-chord)/CONFIG["transition_scale_m"]
                emission=np.array([c["offset"]**2/(2*CONFIG["gps_sigma_m"]**2) for c in candidates[end]])
                next_cost=emission+np.min(reachable[:,None]+transition,axis=0)
                if not np.isfinite(next_cost).any():
                    break
                reachable=next_cost
                transitions.append(transition)
                emissions.append(emission)
                end+=1
            path,pairs=solve_chain(emissions,transitions)
            chosen[start:end]=path
            for j,margins in enumerate(pairs):
                for ai,bi in np.argwhere(margins<=CONFIG["accepted_margin"]+1e-9):
                    length,pieces=self.route_pieces(candidates[start+j][ai],candidates[start+j+1][bi])
                    alternatives[start+j].append((length,pieces))
            start=end
        return xy,candidates,chosen,alternatives

    def composition(self,pieces,tag):
        lengths=Counter()
        for edge,a,b in pieces:
            way=self.ways[self.edges[edge][3]]
            lengths[way["tags"].get(tag,"unknown")]+=abs(b-a)
        total=sum(lengths.values())
        return {value:length/total for value,length in lengths.items()} if total else {"unknown":1.0}
