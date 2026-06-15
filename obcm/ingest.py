import osmium
from shapely.geometry import LineString, Polygon

class OSMHandler(osmium.SimpleHandler):
    def __init__(self, config):
        super().__init__()
        self.config = config
        self.features = []
        self.coastlines = []

    def _get_style(self, tags):
        for tag_key, tag_val in tags:
            if tag_key in self.config["features"] and tag_val in self.config["features"][tag_key]:
                return self.config["features"][tag_key][tag_val]
        return None

    def way(self, w):
        # Catch coastlines for sea generation
        if w.tags.get("natural") == "coastline":
            try:
                coords = [(n.lon, n.lat) for n in w.nodes]
                if len(coords) >= 2:
                    self.coastlines.append(LineString(coords))
            except osmium.InvalidLocationError:
                pass
            return

        style = self._get_style(w.tags)
        if not style: return

        # Prevent closed ways from being added twice (once in way() and once in area()).
        # Closed ways that are tagged as areas should only be processed in area().
        if w.is_closed():
            is_area = w.tags.get("area") == "yes"
            # Common tags that imply an area if the way is closed
            area_tags = {"building", "landuse", "amenity", "leisure", "natural"}
            if not is_area:
                for k, v in w.tags:
                    if k in area_tags:
                        is_area = True
                        break
            
            # area=no overrides everything
            if w.tags.get("area") == "no":
                is_area = False
                
            if is_area:
                return

        # If it's closed and NOT explicitly marked as an area by AreaManager, it might just be a circular road.
        # AreaManager handles true multipolygons.
        try:
            coords = [(n.lon, n.lat) for n in w.nodes]
            if len(coords) >= 2:
                self.features.append({
                    "style_id": style["id"],
                    "geometry": LineString(coords)
                })
        except osmium.InvalidLocationError:
            pass

    def area(self, a):
        style = self._get_style(a.tags)
        if not style: return
        
        try:
            for outer in a.outer_rings():
                ext_coords = [(n.lon, n.lat) for n in outer]
                interiors = []
                for inner in a.inner_rings(outer):
                    interiors.append([(n.lon, n.lat) for n in inner])
                
                if len(ext_coords) >= 3:
                    # Polygons must be closed
                    if ext_coords[0] != ext_coords[-1]:
                        ext_coords.append(ext_coords[0])
                    
                    closed_interiors = []
                    for inner in interiors:
                        if len(inner) >= 3:
                            if inner[0] != inner[-1]:
                                inner.append(inner[0])
                            closed_interiors.append(inner)
                            
                    self.features.append({
                        "style_id": style["id"],
                        "geometry": Polygon(ext_coords, closed_interiors)
                    })
        except osmium.InvalidLocationError:
            pass

def ingest_osm(pbf_path, config):
    handler = OSMHandler(config)
    
    # We must use a NodeLocationsForWays to resolve locations before AreaManager
    import osmium.index
    idx = osmium.index.create_map('flex_mem')
    lh = osmium.NodeLocationsForWays(idx)
    lh.ignore_errors()
    
    # AreaManager handles relations and closed ways tagged as areas
    am = osmium.AreaManager()
    
    # Pass 1: Collect relations
    print("Pass 1: Reading relations...")
    r = osmium.io.Reader(pbf_path, osmium.osm.osm_entity_bits.RELATION)
    am.read_relations(r)
    r.close()
    
    # Pass 2: Build areas and process ways
    print("Pass 2: Processing ways and areas...")
    r = osmium.io.Reader(pbf_path)
    osmium.apply(r, lh, am, handler)
    r.close()
    
    return handler.features, handler.coastlines
