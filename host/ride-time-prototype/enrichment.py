"""Attach local OSM attributes to a fixed development-only ride pilot."""

import argparse
from collections import Counter
from datetime import datetime, timezone
import gzip
import hashlib
import json
from pathlib import Path
import sqlite3
import time
import zlib

import numpy as np
import osmium

from data import CACHE, valid_mask
from enrichment_data import load_track
from enrichment_match import Network, SETTINGS, TAGS, interval_status

DEFAULT_OUTPUT = Path(".artifacts/ride-time-enrichment")


def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f,"sha256").hexdigest()


def prepare_osm(source, output):
    selection = json.loads((output/"selection.json").read_text())
    west,south,east,north = selection["bbox"]
    bbox = [west-0.025,south-0.025,east+0.025,north+0.025]
    ways = []

    class Handler(osmium.SimpleHandler):
        def way(self, way):
            highway = way.tags.get("highway")
            if not highway or way.tags.get("area")=="yes" or highway in {"proposed","construction","abandoned","motorway","motorway_link"}:
                return
            if len(way.nodes) < 2 or any(not n.location.valid() for n in way.nodes):
                return
            coords = [(n.lon,n.lat) for n in way.nodes]
            xs,ys = zip(*coords)
            if max(xs)<bbox[0] or min(xs)>bbox[2] or max(ys)<bbox[1] or min(ys)>bbox[3]:
                return
            ways.append(dict(id=way.id,nodes=[n.ref for n in way.nodes],coordinates=coords,
                             tags={key:way.tags[key] for key in TAGS if key in way.tags}))

    Handler().apply_file(str(source),locations=True,idx="flex_mem")
    reader = osmium.io.Reader(str(source))
    header = reader.header()
    stamp = header.get("osmosis_replication_timestamp")
    reader.close()
    with gzip.open(output/"ways.json.gz","wt") as f:
        json.dump(ways,f,separators=(",",":"))
    provenance = dict(source_url=f"https://download.geofabrik.de/europe/{source.name}",
                      source_sha256=digest(source),source_bytes=source.stat().st_size,
                      osm_timestamp=stamp,extracted_bbox=bbox,ways=len(ways),
                      attribution="© OpenStreetMap contributors; ODbL 1.0. Extract by Geofabrik.")
    (output/"osm-provenance.json").write_text(json.dumps(provenance,indent=2)+"\n")
    print(json.dumps(provenance,indent=2),flush=True)


def audit(output):
    start = time.perf_counter()
    selection = json.loads((output/"selection.json").read_text())
    with gzip.open(output/"ways.json.gz","rt") as f:
        ways = json.load(f)
    west,south,east,north = selection["bbox"]
    network = Network(ways,[(west+east)/2,(south+north)/2])
    print(f"Graph: {len(ways)} ways, {len(network.edges)} segments",flush=True)
    results = []
    private = output/"matches"
    private.mkdir(exist_ok=True)
    with sqlite3.connect(CACHE/"fitrec.sqlite") as db:
        for number,item in enumerate(selection["tracks"]):
            track = load_track(CACHE/"development-tracks.sqlite",item["ride"])
            points,blob = db.execute("SELECT points,data FROM rides WHERE ride=?",(item["ride"],)).fetchone()
            values = np.frombuffer(zlib.decompress(blob),dtype="<f4").reshape(4,points-1)
            xy,candidates,chosen,margins,connected = network.match(track)
            valid = valid_mask(values)
            statuses,tag_lengths = Counter(),{tag:Counter() for tag in TAGS}
            status_rows,offsets,nearest_different = [],[],0
            matched_ways,matched_edges = [],[]
            rawtag_rows = []
            for i,(km,ok) in enumerate(zip(values[0],valid)):
                a = candidates[i][chosen[i]] if chosen[i]>=0 else None
                b = candidates[i+1][chosen[i+1]] if chosen[i+1]>=0 else None
                length,pieces = network.route(a,b,True) if a and b else (float("inf"),[])
                status = interval_status(a,b,margins[i],margins[i+1],connected[i],ok,float(km)*1000,length)
                status_rows.append(status)
                statuses[status] += float(km)
                fractions = {}
                if status == "accepted":
                    offsets.extend([a["offset"],b["offset"]])
                    nearest_different += int(a["way"] != candidates[i][0]["way"])
                    nearest_different += int(b["way"] != candidates[i+1][0]["way"])
                    for tag in TAGS:
                        mix = Counter()
                        for wi,metres in pieces:
                            mix[ways[wi]["tags"].get(tag,"unknown")] += metres/length
                        fractions[tag] = dict(mix)
                        for value,fraction in mix.items():
                            tag_lengths[tag][value] += float(km)*fraction
                rawtag_rows.append(fractions)
                matched_ways.append(ways[a["way"]]["id"] if a else -1)
                matched_edges.append(a["edge"] if a else -1)
            result = dict(**item,points=len(track),raw_km=float(values[0].sum()),screened_km=float(values[0,valid].sum()),
                          status_km=dict(statuses),tag_km={k:dict(v) for k,v in tag_lengths.items()},
                          accepted_offset_median=float(np.median(offsets)) if offsets else None,
                          accepted_endpoint_occurrences=len(offsets),nearest_different=nearest_different)
            results.append(result)
            (private/f"{number:02d}.json").write_text(json.dumps(dict(index=number,status=status_rows,
                selected_way=matched_ways,selected_edge=matched_edges,tag_fractions=rawtag_rows),separators=(",",":")))
            print(f"Ride {number+1:02d}/{len(selection['tracks'])}: {item['sport']}; "
                  f"{statuses['accepted']:.1f}/{result['raw_km']:.1f} km pass matching checks",flush=True)
    groups = {}
    for name,rows in (("all",results),("MTB",[r for r in results if r["sport"]=="mountain bike"]),
                      ("other cycling",[r for r in results if r["sport"]!="mountain bike"])):
        status = sum((Counter(r["status_km"]) for r in rows),Counter())
        tags = {tag:dict(sum((Counter(r["tag_km"][tag]) for r in rows),Counter())) for tag in TAGS}
        groups[name] = dict(rides=len(rows),riders=len({r["user"] for r in rows}),
                           raw_km=sum(r["raw_km"] for r in rows),screened_km=sum(r["screened_km"] for r in rows),
                           status_km=dict(status),tag_km=tags,
                           median_ride_match_fraction=float(np.median([r["status_km"].get("accepted",0)/r["raw_km"] for r in rows])),
                           nearest_different=sum(r["nearest_different"] for r in rows),
                           accepted_endpoint_occurrences=sum(r["accepted_endpoint_occurrences"] for r in rows))
    stamp = [datetime.fromtimestamp(r["start"],timezone.utc).date().isoformat() for r in results]
    summary = dict(groups=groups,settings=SETTINGS,selection={k:v for k,v in selection.items() if k!="tracks"},
                   ride_dates=[min(stamp),max(stamp)],osm=json.loads((output/"osm-provenance.json").read_text()),
                   graph=dict(ways=len(ways),segments=len(network.edges),nodes=len(network.adj)),
                   runtime_seconds=time.perf_counter()-start,
                   source_hashes={p.name:digest(p) for p in [Path(__file__),Path(__file__).with_name("enrichment_match.py"),
                                  Path(__file__).with_name("enrichment_data.py"),output/"selection.json"]})
    (output/"ride-results.json").write_text(json.dumps(results,indent=2)+"\n")
    (output/"summary.json").write_text(json.dumps(summary,indent=2)+"\n")
    print(f"Audit complete in {summary['runtime_seconds']:.1f} s",flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage",choices=("osm","audit"))
    parser.add_argument("--output",type=Path,default=DEFAULT_OUTPUT)
    parser.add_argument("--source",type=Path,default=CACHE/"denmark-260913.osm.pbf")
    args = parser.parse_args()
    if args.stage == "osm":
        prepare_osm(args.source,args.output)
    else:
        audit(args.output)
