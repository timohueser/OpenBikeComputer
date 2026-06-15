import osmium
from shapely.geometry import LineString

class OSMHandler(osmium.SimpleHandler):
    def __init__(self, config):
        super().__init__()
        self.config = config
        self.features = []

    def way(self, w):
        for key, values in self.config["features"].items():
            if key in w.tags and w.tags[key] in values:
                style = values[w.tags[key]]
                try:
                    # Extract coordinates from nodes
                    coords = [(n.lon, n.lat) for n in w.nodes]
                    if len(coords) < 2:
                        return
                    self.features.append({
                        "style_id": style["id"],
                        "geometry": LineString(coords)
                    })
                except osmium.InvalidLocationError:
                    return

def ingest_osm(pbf_path, config):
    handler = OSMHandler(config)
    # Use locations=True to resolve node references to coordinates
    handler.apply_file(pbf_path, locations=True)
    return handler.features
