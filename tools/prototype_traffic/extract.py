"""Reads drivable roads, places and buildings inside a box from OSM extracts into one npz file."""
import sys, math, os
import numpy as np
import osmium

DRIVE = {"motorway", "trunk", "primary", "secondary", "tertiary", "unclassified", "residential",
         "living_street", "motorway_link", "trunk_link", "primary_link", "secondary_link", "tertiary_link"}
CLASSES = ["motorway", "trunk", "primary", "secondary", "tertiary", "unclassified", "residential", "living_street"]
PLACES = ["city", "town", "village", "hamlet", "suburb", "isolated_dwelling"]
IGNORE = {"garage", "garages", "shed", "barn", "farm_auxiliary", "roof", "greenhouse", "hut", "stable", "cowshed", "carport",
          "service", "storage_tank", "silo", "ruins", "construction", "bunker", "boathouse", "toilets", "transformer_tower"}
WORK = {"commercial", "retail", "industrial", "office", "warehouse", "supermarket", "school", "hospital", "university",
        "college", "public", "civic", "government", "manufacture", "factory", "hotel", "kindergarten", "train_station", "sports_hall"}


def num(v, mph_ok=False):
    if not v:
        return 0.0
    v = v.strip().split(";")[0]
    try:
        if mph_ok and v.endswith("mph"):
            return float(v[:-3]) * 1.609
        return float(v.replace("km/h", "").replace("m", "").strip())
    except ValueError:
        return 0.0


class H(osmium.SimpleHandler):
    def __init__(self, box):
        super().__init__()
        self.box = box
        self.way_nodes, self.way_start, self.lon, self.lat = [], [0], [], []
        self.attr, self.way_ref, self.way_id, self.places, self.bld = [], [], [], [], []

    def inside(self, lon, lat):
        b = self.box
        return b[0] <= lon <= b[2] and b[1] <= lat <= b[3]

    def node(self, n):
        p = n.tags.get("place")
        if p in PLACES and self.inside(n.location.lon, n.location.lat):
            self.places.append((n.location.lon, n.location.lat, PLACES.index(p), num(n.tags.get("population", "").replace(",", ""))))

    def way(self, w):
        t = w.tags
        if "building" in t:
            kind = 0 if t["building"] in IGNORE else 2 if t["building"] in WORK else 1
            if kind == 0:
                return
            try:
                if not self.inside(w.nodes[0].lon, w.nodes[0].lat):
                    return
                pts = [(n.lon, n.lat) for n in w.nodes]
            except (osmium.InvalidLocationError, IndexError):
                return
            if len(pts) < 4:
                return
            c = math.cos(math.radians(pts[0][1]))
            area = 0.5 * abs(sum((x0 * y1 - x1 * y0) for (x0, y0), (x1, y1) in zip(pts, pts[1:]))) * c * 111320 * 110574
            self.bld.append((pts[0][0], pts[0][1], min(area, 50000.0), kind))
            return
        h = t.get("highway")
        if h not in DRIVE or t.get("access") in ("no", "private") or t.get("motor_vehicle") in ("no", "private") or t.get("area") == "yes":
            return
        try:
            pts = [(n.ref, n.lon, n.lat) for n in w.nodes]
        except osmium.InvalidLocationError:
            return
        if not any(self.inside(lo, la) for _, lo, la in pts):
            return
        for ref, lo, la in pts:
            self.way_nodes.append(ref); self.lon.append(lo); self.lat.append(la)
        self.way_start.append(len(self.way_nodes))
        ow = t.get("oneway")
        lm = t.get("lane_markings")
        surf = t.get("surface", "")
        try:
            lanes = int(t.get("lanes", "0"))
        except ValueError:
            lanes = 0
        self.attr.append((CLASSES.index(h.replace("_link", "")) + (10 if h.endswith("_link") else 0),
                          num(t.get("maxspeed"), True), lanes,
                          1 if ow in ("yes", "1", "true") or t.get("junction") == "roundabout" or h.startswith("motorway") else (-1 if ow == "-1" else 0),
                          num(t.get("width")), 1 if lm == "no" else 2 if lm == "yes" else 0,
                          0 if not surf else 1 if surf in ("asphalt", "paved", "concrete", "chipseal", "paving_stones", "sett") else 2,
                          1 if t.get("lit") == "yes" else 0))
        self.way_ref.append(t.get("ref", ""))
        self.way_id.append(w.id)


box = [float(x) for x in sys.argv[2].split(",")]  # lon_min,lat_min,lon_max,lat_max
h = H(box)
for path in sys.argv[3:]:
    # A large file keeps node locations on disk; a dense index in memory would not fit.
    idx = f"sparse_file_array,{sys.argv[1]}.nodes" if os.path.getsize(path) > 2e9 else "flex_mem"
    h.apply_file(path, locations=True, idx=idx, filters=[osmium.filter.KeyFilter("place", "building", "highway")])
    if os.path.exists(f"{sys.argv[1]}.nodes"):
        os.remove(f"{sys.argv[1]}.nodes")
    print(path, "ways", len(h.way_id), "buildings", len(h.bld), flush=True)
A = np.array(h.attr, np.float32)
_, keep = np.unique(np.array(h.way_id), return_index=True)  # a way in two extracts counts once
keep = np.sort(keep)
ws = np.array(h.way_start)
idx = np.concatenate([np.arange(ws[i], ws[i + 1]) for i in keep])
lens = ws[keep + 1] - ws[keep]
bld = np.unique(np.array(h.bld, np.float64), axis=0)
np.savez_compressed(sys.argv[1], way_nodes=np.array(h.way_nodes, np.int64)[idx], lon=np.array(h.lon)[idx], lat=np.array(h.lat)[idx],
                    way_start=np.r_[0, np.cumsum(lens)], way_cls=A[keep, 0].astype(np.int8), way_speed=A[keep, 1], way_lanes=A[keep, 2].astype(np.int8),
                    way_oneway=A[keep, 3].astype(np.int8), way_width=A[keep, 4], way_marking=A[keep, 5].astype(np.int8),
                    way_surface=A[keep, 6].astype(np.int8), way_lit=A[keep, 7].astype(np.int8),
                    way_ref=np.array(h.way_ref)[keep], way_id=np.array(h.way_id)[keep], places=np.unique(np.array(h.places), axis=0), bld=bld)
print("ways", len(keep), "places", len(h.places), "buildings", len(bld))
