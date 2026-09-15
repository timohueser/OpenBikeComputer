"""Aggregate report and private map overlays for the OSM enrichment pilot."""

import argparse
import base64
from collections import Counter
import gzip
import html
import json
from pathlib import Path
import shutil
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.collections import LineCollection
import numpy as np
from shapely import box

from data import CACHE
from enrichment_data import load_track
from enrichment_match import Network, TAGS, project

ROOT = Path(__file__).resolve().parents[2]
COLORS = ["#305e4c","#d6c8b5","#b8583f","#c89132","#708fa3","#8c7087","#777777"]


def table(headers,rows):
    return "| "+" | ".join(headers)+" |\n| "+" | ".join(["---"]*len(headers))+" |\n"+"\n".join(
        "| "+" | ".join(map(str,row))+" |" for row in rows)+"\n"


def figure(fig,output,name):
    fig.tight_layout()
    fig.savefig(output/f"{name}.svg",bbox_inches="tight")
    fig.savefig(output/f"{name}.png",dpi=150,bbox_inches="tight")
    plt.close(fig)


def charts(summary,output):
    names = ["MTB","other cycling"]
    statuses = [("accepted","Passes matching checks"),("timing_or_geometry_screen","Recording screen"),
                ("no_candidate","No nearby candidate"),("ambiguous","Ambiguous way"),
                ("large_offset","Large GPS offset"),("length_disagreement","Distance disagreement"),
                ("disconnected_or_gap","Disconnected or gap")]
    fig,ax = plt.subplots(figsize=(10,4.7))
    left = np.zeros(2)
    for (key,label),color in zip(statuses,COLORS):
        values = np.array([100*summary["groups"][n]["status_km"].get(key,0)/summary["groups"][n]["raw_km"] for n in names])
        ax.barh(names,values,left=left,label=label,color=color)
        left += values
    ax.set(xlim=(0,100),xlabel="Share of original GPS chord distance (%)",title="How much recorded distance can receive OSM attributes?")
    ax.legend(loc="upper center",bbox_to_anchor=(0.5,-0.17),ncol=3,fontsize=9)
    figure(fig,output,"matching-coverage")
    fig,ax = plt.subplots(figsize=(10,4.7))
    for j,name in enumerate(names):
        group = summary["groups"][name]
        accepted = group["status_km"].get("accepted",0)
        values = [100*(accepted-group["tag_km"][tag].get("unknown",0))/accepted for tag in TAGS[1:]]
        ax.barh(np.arange(5)+(j-0.5)*0.34,values,height=0.34,label=name,color=COLORS[j*2])
    ax.set(yticks=np.arange(5),yticklabels=TAGS[1:],xlim=(0,100),xlabel="Share of accepted distance with an explicit tag (%)",
           title="Matching a trail does not mean its properties are mapped")
    ax.legend(loc="lower right")
    figure(fig,output,"tag-coverage")


def examples(output):
    selection = json.loads((output/"selection.json").read_text())
    with gzip.open(output/"ways.json.gz","rt") as f:
        ways = json.load(f)
    w,s,e,n = selection["bbox"]
    network = Network(ways,[(w+e)/2,(s+n)/2])
    cases = {}
    for ri,item in enumerate(selection["tracks"]):
        match = json.loads((output/"matches"/f"{ri:02d}.json").read_text())
        mtb = item["sport"]=="mountain bike"
        for i,(status,tags) in enumerate(zip(match["status"],match["tag_fractions"])):
            label = None
            if mtb and status=="accepted" and tags["highway"].get("path",0)>0.5:
                label = "MTB: accepted path"
            elif mtb and status=="no_candidate":
                label = "MTB: no nearby candidate"
            elif mtb and status=="ambiguous":
                label = "MTB: ambiguous way"
            elif mtb and status=="accepted" and tags["mtb:scale"].get("unknown",0)<0.5:
                label = "MTB: explicit difficulty tag"
            elif not mtb and status=="accepted" and tags["surface"].get("asphalt",0)>0.5:
                label = "Other cycling: accepted asphalt"
            elif mtb and status=="length_disagreement":
                label = "MTB: distance disagreement"
            if label and label not in cases and 15<i<len(match["status"])-15:
                cases[label]=(ri,i)
    descriptions = []
    for ci,(label,(ri,i)) in enumerate(cases.items()):
        item = selection["tracks"][ri]
        track = load_track(CACHE/"development-tracks.sqlite",item["ride"])
        xy = project(track[:,:2],network.origin)
        center = xy[i:i+2].mean(axis=0)
        bounds = box(*(center-[500,500]),*(center+[500,500]))
        edges = network.tree.query(bounds,predicate="intersects")
        match = json.loads((output/"matches"/f"{ri:02d}.json").read_text())
        selected = {x for x in match["selected_edge"][max(0,i-15):i+16] if x>=0}
        fig,ax = plt.subplots(figsize=(8,7))
        ax.add_collection(LineCollection([np.array(network.lines[k].coords)-center for k in edges],colors="#b6b6b6",linewidths=1))
        ax.add_collection(LineCollection([np.array(network.lines[k].coords)-center for k in selected],colors=COLORS[0],linewidths=3))
        shown = xy[max(0,i-15):i+17]-center
        ax.plot(shown[:,0],shown[:,1],"o-",color="#3278af",markersize=3,linewidth=1,label="Recorded GPS samples and chords")
        ax.scatter(*(xy[i]-center),s=130,facecolors="none",edgecolors="#b8583f",linewidths=2,label="Inspected interval start")
        ax.set(xlim=(-500,500),ylim=(-500,500),aspect="equal",xlabel="Metres east of example centre",ylabel="Metres north",
               title=label+"\nGreen: selected candidate segments; grey: OSM ways")
        ax.legend(loc="lower left",fontsize=8)
        fig.text(0.02,0.01,"Private ride overlay. Map data © OpenStreetMap contributors (ODbL), via Geofabrik.",fontsize=7)
        name=f"example-{ci+1}"
        figure(fig,output,name)
        descriptions.append(dict(title=label,ride_index=ri,interval=i,image=name+".png",status=match["status"][i],
                                 tags=match["tag_fractions"][i]))
    (output/"examples.json").write_text(json.dumps(descriptions,indent=2)+"\n")
    print(f"Generated {len(descriptions)} private example overlays")


def report(output,publish):
    x = json.loads((output/"summary.json").read_text())
    source = json.loads((CACHE/"fitrec.audit.json").read_text())
    x["fitrec"] = {key:source[key] for key in ("source_url","source_sha256","source_bytes")}
    charts(x,output)
    groups = x["groups"]
    rows = []
    for name,g in groups.items():
        a = g["status_km"].get("accepted",0)
        rows.append([name,g["rides"],g["riders"],f"{g['raw_km']:.1f}",f"{a:.1f}",
                     f"{100*a/g['raw_km']:.1f}%",f"{100*a/g['screened_km']:.1f}%"])
    coverage = []
    for tag in TAGS:
        row = [f"`{tag}`"]
        for name in ("MTB","other cycling"):
            g = groups[name]
            a = g["status_km"].get("accepted",0)
            known = a-g["tag_km"][tag].get("unknown",0)
            row.extend([f"{100*known/a:.1f}%",f"{100*known/g['raw_km']:.1f}%"])
        coverage.append(row)
    notes_path = output/"inspection.json"
    notes = json.loads(notes_path.read_text()) if notes_path.exists() else {"findings":["Visual inspection is pending."]}
    mtb = groups["MTB"]
    accepted_mtb = mtb["status_km"]["accepted"]
    surface_mtb = accepted_mtb-mtb["tag_km"]["surface"].get("unknown",0)
    rating_mtb = accepted_mtb-mtb["tag_km"]["mtb:scale"].get("unknown",0)
    text = ["# OSM surface enrichment: feasibility pilot\n",
        "**Status: development-only feasibility audit.** This experiment measures map association and attribute availability. "
        "It does not measure an improvement in ETA accuracy. A match that passes the checks is not a verified trail label.\n",
        "## Findings\n",
        f"- **Surface enrichment is possible:** {100*surface_mtb/accepted_mtb:.1f}% of accepted MTB distance has an explicit surface tag.\n"
        f"- **Matching is the main current bottleneck:** {100*accepted_mtb/mtb['raw_km']:.1f}% of recorded MTB distance passes the conservative checks; "
        f"{100*mtb['status_km'].get('ambiguous',0)/mtb['raw_km']:.1f}% is rejected for ambiguity between OSM ways. "
        "That is a limit of this matcher and screen, not a measurement of missing OSM paths.\n"
        f"- **Technical ratings are too sparse to require:** `mtb:scale` covers {100*rating_mtb/accepted_mtb:.1f}% of accepted MTB distance, "
        f"or {100*rating_mtb/mtb['raw_km']:.1f}% of all recorded MTB distance.\n"
        f"- Combining matching and tag availability, explicit surface labels cover **{100*surface_mtb/mtb['raw_km']:.1f}% of recorded MTB distance**. "
        "Coverage within accepted matches alone would overstate the amount of usable evidence.\n",
        "## 1. Scope and selection\n",
        f"The pilot contains **80 rides from {groups['all']['riders']} riders**, including 40 MTB rides, entirely inside "
        "9–10°E and 56–57°N in central Jutland, Denmark. A survey of the 18,929 eligible development histories ranked one-degree "
        "cells by distinct MTB riders. This cell ranked first. After requiring complete containment, 364 rides from 33 riders "
        "were eligible, including 138 MTB rides. Selection uses stable hashes and balances riders within 40 MTB and 40 other "
        "cycling slots. It does not use prediction errors, tag presence or match success.\n\n"
        f"Ride dates range from **{x['ride_dates'][0]} to {x['ride_dates'][1]}**. The OSM snapshot is "
        f"**{x['osm']['osm_timestamp']}**. This 10–14 year mismatch can change geometry, connectivity and attributes. "
        "Current tags do not establish historical trail conditions. The geographic selection is useful for this pilot, but "
        "does not represent mountain terrain or worldwide mapping coverage.\n",
        "## 2. Matching coverage\n",
        "![Matching coverage and rejection reasons](matching-coverage.svg)\n",
        table(["Activity","Rides","Riders","Recorded km","Accepted km","Accepted / recorded","Accepted / screened"],rows),
        "Distances use the original GPS chords. The screened denominator contains only intervals accepted by the original "
        "30-second timing and geometry screen. The recorded denominator also includes excluded intervals. These are pooled "
        "distance shares in a deliberately balanced pilot, not population estimates.\n",
        "## 3. Availability of explicit attributes\n",
        "![Explicit tag coverage on accepted distance](tag-coverage.svg)\n",
        table(["Tag","MTB / accepted","MTB / recorded","Other / accepted","Other / recorded"],coverage),
        "An absent tag stays **unknown**. No surface is inferred from activity label or road class. Generic `unpaved` remains "
        "distinct from gravel. A missing `mtb:scale` does not mean easy. For intervals crossing several ways, tags are weighted "
        "by reconstructed path length, then allocated to that interval's original GPS chord distance. This is a distance "
        "allocation, not a claim about time spent on each surface.\n",
        "### Most common surface values on accepted distance\n",
        table(["Activity","Surface","Distance km","Share of accepted"],[
            [name,value,f"{km:.1f}",f"{100*km/g['status_km']['accepted']:.1f}%"]
            for name,g in groups.items() if name!="all"
            for value,km in sorted(g["tag_km"]["surface"].items(),key=lambda p:-p[1])[:8]]),
        "### Most common road/path types on accepted distance\n",
        table(["Activity","OSM highway type","Distance km"],[
            [name,value,f"{km:.1f}"] for name,g in groups.items() if name!="all"
            for value,km in sorted(g["tag_km"]["highway"].items(),key=lambda p:-p[1])[:7]]),
        "## 4. Matching method and its limits\n",
        "The graph contains line ways with `highway` tags. Area polygons, motorway ways and proposed, construction or abandoned "
        "highways are excluded. Other access and one-way restrictions are not enforced: the task reconstructs observed travel. "
        "A buffered bounding box limits extraction. No GPS coordinates are sent to a matching service.\n\n"
        "At each sample, the matcher considers up to eight nearby segments within 40 m, at most two per OSM way. "
        "A minimum-cost sequence combines GPS offset with the difference between connected-path length and GPS chord length. "
        "No learned speed or surface preference enters the matching cost. Gaps above 30 seconds, nonpositive time steps, "
        "large jumps and disconnected paths split the sequence.\n\n"
        "An accepted interval must pass the existing recording screen, have endpoint offsets at most 20 m, and have an "
        "alternative-way cost margin of at least three at both endpoints. Matched length must be 0.5–1.5 times chord length "
        "and differ by at most 60 m. The margin is a heuristic, not a calibrated probability. Missing mapped paths or "
        "discarded candidates can still produce an incorrect accepted match. Segment identity within one OSM way is not "
        "treated as a different-way alternative.\n\n"
        "Offline sequence matching uses later coordinates within each continuous block. This audit therefore does not "
        "validate a causal device matcher or a live ETA replay with enriched attributes. A later ETA experiment must use "
        "a genuinely known planned route and causal observation features.\n",
        f"The extracted graph contains {x['graph']['ways']:,} ways and {x['graph']['segments']:,} segments. "
        f"Matching and audit took {x['runtime_seconds']:.1f} seconds on the host. This is not an nRF54 runtime estimate.\n",
        "## 5. Visual spot checks\n",
        "Example overlays show recorded samples, candidate segments and nearby OSM geometry. They are private artifacts "
        "because they contain source ride geometry. Cases are selected to illustrate acceptance and failure modes, not "
        "to estimate a match-error rate. Geometry inspection alone cannot verify surface material.\n",
        "\n".join("- "+finding for finding in notes["findings"])+"\n",
        "## 6. Next decision\n",
        notes.get("decision","Interpretation is pending the visual checks.")+"\n",
        "Before an ETA comparison, use development data to define a small surface/path model and an uncertainty policy "
        "for unknown attributes. Compare models on identical issued targets and report both tag coverage and forecast "
        "accuracy. Reusing inspected test riders for tuning would not provide a new independent evaluation. Dense original "
        "rides remain necessary for moving-time labels, pushing and long forecasts.\n",
        "## 7. Reproduction and provenance\n",
        "The [prototype README](src:host/ride-time-prototype/README.md) gives commands. "
        "[OSM source extract]("+x['osm']['source_url']+") · "
        "[FitRec source](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html).\n\n"
        "Map data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright), ODbL 1.0; extract by Geofabrik. "
        "The OSM source hash, extraction bounds, matching settings and source-code hashes are in the aggregate JSON. "
        "Raw archives, selected rider identifiers, coordinates and per-interval attributes remain outside tracked source.\n"]
    markdown = "\n".join(text)
    (output/"report.md").write_text(markdown)
    sys.path.insert(0,str(ROOT/"docs"))
    from build_docs import render_blocks
    rendered, _ = render_blocks(markdown)
    for name in ("matching-coverage","tag-coverage"):
        data = base64.b64encode((output/f"{name}.svg").read_bytes()).decode()
        rendered = rendered.replace(f'src="{name}.svg"',f'src="data:image/svg+xml;base64,{data}"')
    if (output/"examples.json").exists():
        rendered += "<h2>Private example overlays</h2><p>Do not redistribute these source ride geometries.</p>"
        for case in json.loads((output/"examples.json").read_text()):
            encoded = base64.b64encode((output/case["image"]).read_bytes()).decode()
            rendered += f'<h3>{html.escape(case["title"])}</h3><img alt="{html.escape(case["title"])}" src="data:image/png;base64,{encoded}">'
    (output/"report.html").write_text('<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
        '<title>OSM enrichment feasibility</title><style>body{max-width:1050px;margin:40px auto;padding:0 20px;font:17px/1.6 system-ui;color:#243d32;background:#faf8f2}'
        'h1,h2,h3{line-height:1.25}h2{margin-top:2em}img{max-width:100%;height:auto}table{display:block;overflow:auto;border-collapse:collapse;font-size:14px}'
        'th,td{padding:7px 12px;border-bottom:1px solid #d6c8b5;text-align:left}a{color:#236e58}code{font-size:.9em}</style><body>'+rendered+'</body></html>')
    if publish:
        assets = ROOT/"docs/assets/research/ride-time"
        assets.mkdir(parents=True,exist_ok=True)
        for name in ("matching-coverage","tag-coverage"):
            shutil.copyfile(output/f"{name}.svg",assets/f"{name}.svg")
            markdown = markdown.replace(f"]({name}.svg)",f"](/assets/research/ride-time/{name}.svg)")
        (ROOT/"docs/content/software/ride-time-enrichment.md").write_text("---\ncopy: ai\n---\n\n"+markdown)
        aggregate = dict(x,inspection=notes)
        (ROOT/"host/ride-time-prototype/results/enrichment-v1.json").write_text(json.dumps(aggregate,indent=2)+"\n")
    print(f"Report: {output/'report.html'}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage",choices=("examples","report"))
    parser.add_argument("--output",type=Path,default=Path(".artifacts/ride-time-enrichment"))
    parser.add_argument("--publish-docs",action="store_true")
    args=parser.parse_args()
    if args.stage=="examples":
        examples(args.output)
    else:
        report(args.output,args.publish_docs)
