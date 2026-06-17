import osmium
import osmium.index
import osmium.area
from shapely.geometry import LineString, Polygon

class OSMHandler(osmium.SimpleHandler):
    def __init__(self, config):
        super().__init__()
        self.config = config
        self.features = []
        self.coastlines = []
        # Cache the (tag_key, values) pairs once instead of rebuilding the
        # dict items view for every way/area.
        self._feature_defs = list(config.get("features", {}).items())

    def _get_style(self, tags):
        for tag_key, tag_vals in self._feature_defs:
            tag_val = tags.get(tag_key)
            if tag_val is not None and tag_val in tag_vals:
                return tag_vals[tag_val]
        return None

    def way(self, w):
        # Catch coastlines for sea generation
        # ALWAYS do this first, even if closed.
        is_coastline = w.tags.get("natural") == "coastline"
        
        if is_coastline:
            try:
                coords = [(n.lon, n.lat) for n in w.nodes]
                if len(coords) >= 2:
                    self.coastlines.append(LineString(coords).simplify(0.00002, preserve_topology=True))
            except osmium.InvalidLocationError:
                pass
            # Coastlines are NEVER areas for AreaManager, they are just lines.
            # We don't return here because a coastline way could also be tagged as something else
            # (though unlikely in standard OSM).
        
        style = self._get_style(w.tags)
        if not style: return

        # Prevent closed ways from being added twice (once in way() and once in area()).
        # Only skip if it's definitely going to be an area.
        if w.is_closed():
            # osmium.area.AreaManager generally builds areas for these tags
            is_area = False
            if w.tags.get("area") == "yes":
                is_area = True
            elif w.tags.get("area") != "no":
                area_tags = ("building", "landuse", "amenity", "leisure", "natural", "waterway")
                if any(w.tags.get(k) is not None for k in area_tags):
                    is_area = True
            
            if is_area:
                return

        # If it's closed and NOT explicitly marked as an area by AreaManager, it might just be a circular road.
        try:
            coords = [(n.lon, n.lat) for n in w.nodes]
            if len(coords) >= 2:
                self.features.append({
                    "style_id": style["id"],
                    "min_lod": style.get("min_lod", 0),
                    "geometry": LineString(coords).simplify(0.00002, preserve_topology=True)
                })
        except osmium.InvalidLocationError:
            pass

    def area(self, a):
        # Admin boundaries should only be handled as lines
        if "admin_level" in a.tags:
            return
            
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
                        "min_lod": style.get("min_lod", 0),
                        "geometry": Polygon(ext_coords, closed_interiors).simplify(0.00002, preserve_topology=True)
                    })
        except osmium.InvalidLocationError:
            pass

def ingest_osm(pbf_path, config):
    handler = OSMHandler(config)

    # We must use a NodeLocationsForWays to resolve locations before AreaManager
    idx = osmium.index.create_map('flex_mem')
    lh = osmium.NodeLocationsForWays(idx)
    lh.ignore_errors()
    
    # AreaManager handles relations and closed ways tagged as areas
    am = osmium.area.AreaManager()
    
    # Pass 1: Collect relations
    print("Pass 1: Reading relations...")
    r = osmium.io.Reader(pbf_path, osmium.osm.osm_entity_bits.RELATION)
    osmium.apply(r, am.first_pass_handler())
    r.close()
    
    # Pass 2: Build areas and process ways
    print("Pass 2: Processing ways and areas...")
    r = osmium.io.Reader(pbf_path)
    osmium.apply(r, lh, am.second_pass_handler(handler), handler)
    r.close()
    
    return handler.features, handler.coastlines
