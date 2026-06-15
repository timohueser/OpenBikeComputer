import osmium
from shapely.geometry import LineString

class OSMHandler(osmium.SimpleHandler):
    def __init__(self, config):
        super().__init__()
        self.config = config
        self.features = []

    def way(self, w):
        style = None
        for tag_key, tag_val in w.tags:
            if tag_key in self.config["features"] and tag_val in self.config["features"][tag_key]:
                style = self.config["features"][tag_key][tag_val]
                break
        
        if not style:
            return

        try:
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
    # Use flex_mem for efficient node location indexing
    handler.apply_file(pbf_path, locations=True, idx='flex_mem')
    return handler.features
