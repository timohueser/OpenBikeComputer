"""Fixed geometry-only spot checks, rendered without matcher predictions."""

import argparse
import gzip
import hashlib
import json
from pathlib import Path
import sqlite3
import zlib

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.collections import LineCollection
import numpy as np
from shapely import LineString, box

from data import CACHE,valid_mask
from enrichment_data import load_track
from enrichment_match import Network,project

PILOT=Path(".artifacts/ride-time-enrichment")
OUTPUT=Path(".artifacts/ride-time-matching-v2")


def rank(value):
    return hashlib.sha256(f"obc-match-review-v2:{value}".encode()).hexdigest()


def select():
    path=OUTPUT/"review-selection.json"
    if path.exists():
        raise SystemExit("Review selection already exists")
    rides=json.loads((PILOT/"selection.json").read_text())["tracks"]
    inspected={r["user"] for i,r in enumerate(rides) if i in [0,1,3] or 40<=i<50}
    chosen=[]
    for phase in ("development","validation"):
        pool=[(i,r) for i,r in enumerate(rides) if (r["user"] in inspected)==(phase=="development")]
        users=sorted({r["user"] for _,r in pool},key=rank)
        by_user={u:sorted([(i,r) for i,r in pool if r["user"]==u],key=lambda ir:rank(ir[1]["ride"])) for u in users}
        selected=[]
        while len(selected)<20:
            for user in users:
                if by_user[user] and len(selected)<20:
                    selected.append(by_user[user].pop(0))
        with sqlite3.connect(CACHE/"fitrec.sqlite") as db:
            for ri,item in selected:
                points,blob=db.execute("SELECT points,data FROM rides WHERE ride=?",(item["ride"],)).fetchone()
                values=np.frombuffer(zlib.decompress(blob),dtype="<f4").reshape(4,points-1)
                eligible=[int(i) for i in np.flatnonzero(valid_mask(values)) if points*0.2<i<points*0.8]
                index=min(eligible,key=lambda i:rank(f"{item['ride']}:{i}"))
                chosen.append(dict(case=len(chosen)+1,phase=phase,ride_index=ri,interval=index,**item))
    OUTPUT.mkdir(parents=True,exist_ok=True)
    path.write_text(json.dumps(chosen,indent=2)+"\n")
    print(f"Fixed {len(chosen)} sections; validation uses previously uninspected riders")


def render():
    cases=json.loads((OUTPUT/"review-selection.json").read_text())
    with gzip.open(PILOT/"ways.json.gz","rt") as f:ways=json.load(f)
    graph=Network(ways,[9.5,56.5])
    catalogue=[]
    for base in range(0,len(cases),4):
        fig,axes=plt.subplots(2,2,figsize=(18,15))
        for ax,item in zip(axes.flat,cases[base:base+4]):
            track=load_track(CACHE/"development-tracks.sqlite",item["ride"])
            xy=project(track[:,:2],graph.origin)
            i=item["interval"]
            center=xy[i:i+2].mean(axis=0)
            radius=max(100,float(np.linalg.norm(xy[i+1]-xy[i]))/2+70)
            extent=box(*(center-radius),*(center+radius))
            edges=graph.tree.query(extent,predicate="intersects")
            ax.add_collection(LineCollection([np.array(graph.lines[e].coords)-center for e in edges],colors="#b9b9b9",linewidths=1))
            chord=LineString(xy[i:i+2])
            wis={graph.edges[e][3] for e in edges}
            shapes={wi:LineString(project(ways[wi]["coordinates"],graph.origin)) for wi in wis}
            nearest=sorted(wis,key=lambda wi:(shapes[wi].distance(chord),ways[wi]["id"]))[:10]
            labels={}
            for j,wi in enumerate(nearest):
                letter=chr(65+j)
                line=shapes[wi]
                point=line.interpolate(line.project(chord.centroid))
                p=np.array(point.coords[0])-center
                color=plt.cm.tab10(j)
                coords=np.array(line.coords)-center
                ax.plot(coords[:,0],coords[:,1],color=color,linewidth=1.4)
                ax.annotate(letter,p,xytext=(6,6+(j%3)*9),textcoords="offset points",fontsize=11,fontweight="bold",
                            color=color,arrowprops=dict(arrowstyle="-",color=color))
                labels[letter]=dict(way_id=ways[wi]["id"],tags=ways[wi]["tags"])
            shown=xy[max(0,i-6):i+8]-center
            ax.plot(shown[:,0],shown[:,1],"k.-",alpha=0.8,linewidth=1,markersize=7)
            for j in (0,1):
                p=xy[i+j]-center
                ax.scatter(*p,s=100,facecolors="white",edgecolors="black",zorder=10)
                ax.annotate(str(j),p,ha="center",va="center",fontsize=8,zorder=11)
            ax.set(xlim=(-radius,radius),ylim=(-radius,radius),aspect="equal",title=f"Case {item['case']:02d} · {item['phase']} · {item['sport']}\nLabel the path from 0 to 1; black = GPS context")
            ax.set_xlabel("Metres east"); ax.set_ylabel("Metres north")
            # Labels identify candidate ways, without showing model selections or scores.
            description="\n".join(f"{letter}: {v['tags'].get('highway','?')} / {v['tags'].get('surface','?')}" for letter,v in labels.items())
            ax.text(1.02,0.98,description,transform=ax.transAxes,va="top",fontsize=9)
            catalogue.append(dict(**item,labels=labels))
        fig.suptitle("Blind geometry review · OSM © contributors, ODbL · private ride data",fontsize=13)
        fig.tight_layout(rect=(0,0,1,0.96))
        fig.savefig(OUTPUT/f"review-sheet-{base//4+1:02d}.png",dpi=130,bbox_inches="tight")
        plt.close(fig)
    (OUTPUT/"review-catalogue.json").write_text(json.dumps(catalogue,indent=2)+"\n")
    print("Wrote ten review sheets without matcher predictions")


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage",choices=("select","render"))
    args=parser.parse_args()
    select() if args.stage=="select" else render()
