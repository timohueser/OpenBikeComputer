"""Replay the fixed pilot with path and attribute agreement checks."""

from collections import Counter
import gzip
import json
from pathlib import Path
import sqlite3
import time
import zlib

import numpy as np

from data import CACHE,valid_mask
from enrichment import digest
from enrichment_data import load_track
from enrichment_match import TAGS,interval_status
from matching_v2 import CONFIG,PathNetwork,common_composition,same_core,trim
from matching_review import OUTPUT,PILOT


def run():
    start=time.perf_counter()
    selection=json.loads((PILOT/"selection.json").read_text())
    reviews=json.loads((OUTPUT/"review-selection.json").read_text())
    reviewed={(r["ride_index"],r["interval"]):r["case"] for r in reviews}
    with gzip.open(PILOT/"ways.json.gz","rt") as f:ways=json.load(f)
    graph=PathNetwork(ways,[9.5,56.5])
    print("Graph loaded",flush=True)
    rows,review_results=[],[]
    (OUTPUT/"matches").mkdir(exist_ok=True)
    with sqlite3.connect(CACHE/"fitrec.sqlite") as db:
        for ri,item in enumerate(selection["tracks"]):
            track=load_track(CACHE/"development-tracks.sqlite",item["ride"])
            points,blob=db.execute("SELECT points,data FROM rides WHERE ride=?",(item["ride"],)).fetchone()
            values=np.frombuffer(zlib.decompress(blob),dtype="<f4").reshape(4,points-1)
            valid=valid_mask(values)
            xy,candidates,chosen,alternatives=graph.analyse(track)
            totals=Counter(raw_km=float(values[0].sum()),screened_km=float(values[0,valid].sum()))
            tag_totals={tag:Counter() for tag in TAGS}
            intervals=[]
            for i,km in enumerate(values[0]):
                a=candidates[i][chosen[i]] if chosen[i]>=0 else None
                b=candidates[i+1][chosen[i+1]] if chosen[i+1]>=0 else None
                length,pieces=graph.route_pieces(a,b) if a and b else (float("inf"),[])
                status=interval_status(a,b,float("inf"),float("inf"),bool(alternatives[i]),valid[i],float(km)*1000,length)
                entry=dict(status=status,alternatives=len(alternatives[i]),core={},tags={})
                if status=="accepted":
                    totals["plausible_path_km"]+=float(km)
                    routes=[p for _,p in alternatives[i]]
                    for tolerance in (0,5,10,15,20):
                        accepted=all(graph.same_path(pieces,p,tolerance) for p in routes)
                        entry["core"][str(tolerance)]=accepted
                        totals[f"core_{tolerance}_km"]+=float(km)*accepted
                    for tag in TAGS:
                        agreement=common_composition([graph.composition(p,tag) for p in routes])
                        entry["tags"][tag]=agreement
                        for value,fraction in agreement.items():
                            tag_totals[tag][value]+=float(km)*fraction
                else:
                    totals[status+"_km"]+=float(km)
                intervals.append(entry)
                if (ri,i) in reviewed:
                    core=trim(pieces,CONFIG["endpoint_tolerance_m"]) or pieces
                    review_results.append(dict(case=reviewed[(ri,i)],**entry,
                        selected_way_ids=sorted({ways[graph.edges[e][3]]["id"] for e,_,_ in pieces}),
                        core_way_ids=sorted({ways[graph.edges[e][3]]["id"] for e,_,_ in core}),
                        selected_tag_mix={tag:graph.composition(pieces,tag) for tag in TAGS}))
            rows.append(dict(**item,totals=dict(totals),tag_km={k:dict(v) for k,v in tag_totals.items()}))
            (OUTPUT/"matches"/f"{ri:02d}.json").write_text(json.dumps(intervals,separators=(",",":")))
            print(f"Ride {ri+1:02d}/80: plausible {totals['plausible_path_km']/totals['raw_km']:.0%}; "
                  f"core {totals['core_10_km']/totals['raw_km']:.0%}",flush=True)
    groups={}
    for name,group in (("MTB",[r for r in rows if r["sport"]=="mountain bike"]),
                       ("other cycling",[r for r in rows if r["sport"]!="mountain bike"])):
        groups[name]=dict(totals=dict(sum((Counter(r["totals"]) for r in group),Counter())),
            tag_km={tag:dict(sum((Counter(r["tag_km"][tag]) for r in group),Counter())) for tag in TAGS},
            rides=len(group),riders=len({r["user"] for r in group}))
    summary=dict(groups=groups,settings=CONFIG,runtime_seconds=time.perf_counter()-start,
        source_hashes={p.name:digest(p) for p in [Path(__file__),Path(__file__).with_name("matching_v2.py"),
                        OUTPUT/"review-selection.json",PILOT/"selection.json",PILOT/"ways.json.gz"]})
    (OUTPUT/"summary.json").write_text(json.dumps(summary,indent=2)+"\n")
    (OUTPUT/"ride-results.json").write_text(json.dumps(rows,indent=2)+"\n")
    (OUTPUT/"review-predictions.json").write_text(json.dumps(review_results,indent=2)+"\n")
    print(f"Complete in {summary['runtime_seconds']:.1f} s")


if __name__=="__main__":
    run()
