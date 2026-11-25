"""Reads drivable roads, places and building positions from an OSM extract into one npz file."""
import sys
import numpy as np
import osmium

DRIVE = {"motorway", "trunk", "primary", "secondary", "tertiary", "unclassified", "residential",
         "living_street", "motorway_link", "trunk_link", "primary_link", "secondary_link", "tertiary_link"}
CLASSES = ["motorway", "trunk", "primary", "secondary", "tertiary", "unclassified", "residential", "living_street"]


def mph(v):
    if not v:
        return 0.0
    v = v.strip()
    try:
        if v.endswith("mph"):
            return float(v[:-3]) * 1.609
        return float(v.replace("km/h", ""))
    except ValueError:
        return 0.0


class H(osmium.SimpleHandler):
    def __init__(self):
        super().__init__()
        self.way_nodes, self.way_start, self.way_cls, self.way_speed, self.way_lanes = [], [0], [], [], []
        self.way_oneway, self.way_ref, self.way_id = [], [], []
        self.lon, self.lat = [], []
        self.places, self.bld = [], []

    def node(self, n):
        p = n.tags.get("place")
        if p in ("city", "town", "village", "hamlet", "suburb", "isolated_dwelling"):
            pop = n.tags.get("population", "")
            try:
                pop = float(pop.replace(",", ""))
            except ValueError:
                pop = 0.0
            self.places.append((n.location.lon, n.location.lat, ["city", "town", "village", "hamlet", "suburb", "isolated_dwelling"].index(p), pop))

    def way(self, w):
        t = w.tags
        if "building" in t:
            try:
                n = w.nodes[0]
                self.bld.append((n.lon, n.lat))
            except osmium.InvalidLocationError:
                pass
            return
        h = t.get("highway")
        if h not in DRIVE or t.get("access") in ("no", "private") or t.get("motor_vehicle") in ("no", "private") or t.get("area") == "yes":
            return
        try:
            pts = [(n.ref, n.lon, n.lat) for n in w.nodes]
        except osmium.InvalidLocationError:
            return
        for ref, lo, la in pts:
            self.way_nodes.append(ref); self.lon.append(lo); self.lat.append(la)
        self.way_start.append(len(self.way_nodes))
        self.way_cls.append(CLASSES.index(h.replace("_link", "")) + (10 if h.endswith("_link") else 0))
        self.way_speed.append(mph(t.get("maxspeed")))
        try:
            self.way_lanes.append(int(t.get("lanes", "0")))
        except ValueError:
            self.way_lanes.append(0)
        ow = t.get("oneway")
        self.way_oneway.append(1 if ow in ("yes", "1", "true") or t.get("junction") == "roundabout" or h.startswith("motorway") else (-1 if ow == "-1" else 0))
        self.way_ref.append(t.get("ref", ""))
        self.way_id.append(w.id)


h = H()
h.apply_file(sys.argv[1], locations=True, idx="flex_mem")
np.savez_compressed(sys.argv[2], way_nodes=np.array(h.way_nodes, np.int64), lon=np.array(h.lon), lat=np.array(h.lat),
                    way_start=np.array(h.way_start, np.int64), way_cls=np.array(h.way_cls, np.int8),
                    way_speed=np.array(h.way_speed, np.float32), way_lanes=np.array(h.way_lanes, np.int8),
                    way_oneway=np.array(h.way_oneway, np.int8), way_ref=np.array(h.way_ref), way_id=np.array(h.way_id),
                    places=np.array(h.places), bld=np.array(h.bld, np.float32))
print("ways", len(h.way_id), "nodes", len(h.way_nodes), "places", len(h.places), "buildings", len(h.bld))
