from shapely.geometry import box, LineString, MultiLineString

class QuadtreeNode:
    def __init__(self, bbox, chunk_size=4096):
        # bbox: (min_lon, min_lat, max_lon, max_lat) in microdegrees
        self.bbox = bbox
        self.chunk_size = chunk_size
        self.features = []
        self.children = []
        self.is_leaf = True
        
        # Pre-calculate float boundaries and shapely box
        self.min_lon_f, self.min_lat_f, self.max_lon_f, self.max_lat_f = [c / 1e6 for c in self.bbox]
        self.q_box = box(self.min_lon_f, self.min_lat_f, self.max_lon_f, self.max_lat_f)
        
        # Incremental size tracking
        self.current_size = 0

    def insert(self, feature):
        clipped = feature["geometry"].intersection(self.q_box)
        if clipped.is_empty:
            return

        self._flatten_and_process(clipped, feature["style_id"])

    def _flatten_and_process(self, geom, style_id):
        if geom.is_empty:
            return
        
        if hasattr(geom, 'geoms'): # MultiLineString, MultiPolygon, GeometryCollection
            for part in geom.geoms:
                self._flatten_and_process(part, style_id)
        elif geom.geom_type in ['LineString', 'LinearRing', 'Polygon']:
            # Preserve Polygon geometry
            self._process_clipped({"style_id": style_id, "geometry": geom})

    def _process_clipped(self, feature):
        if self.is_leaf:
            self.features.append(feature)
            # Update current_size incrementally
            geom = feature["geometry"]
            if geom.geom_type == 'Polygon':
                pt_count = len(geom.exterior.coords)
                for interior in geom.interiors:
                    pt_count += len(interior.coords)
            else:
                pt_count = len(geom.coords)
                
            # 12 byte header + roughly 4 bytes per point (16-bit deltas)
            self.current_size += 12 + (pt_count * 4)
            
            if self.should_split():
                self.split()
        else:
            for child in self.children:
                child.insert(feature)

    def should_split(self):
        # Physical width limit removed as anchors are now 32-bit.
        # Long segments are dynamically interpolated during serialization to prevent 16-bit delta overflow.
        # Split only if too many points/data
        return self.current_size > self.chunk_size

    def split(self):
        min_lon, min_lat, max_lon, max_lat = self.bbox
        width = max_lon - min_lon
        height = max_lat - min_lat
        
        # Recursion guard: Don't split if smaller than 10 microdegrees
        if width < 10 or height < 10:
            return

        mid_lon = (min_lon + max_lon) // 2
        mid_lat = (min_lat + max_lat) // 2
        
        self.children = [
            QuadtreeNode((min_lon, mid_lat, mid_lon, max_lat), self.chunk_size), # NW
            QuadtreeNode((mid_lon, mid_lat, max_lon, max_lat), self.chunk_size), # NE
            QuadtreeNode((min_lon, min_lat, mid_lon, mid_lat), self.chunk_size), # SW
            QuadtreeNode((mid_lon, min_lat, max_lon, mid_lat), self.chunk_size), # SE
        ]
        self.is_leaf = False
        features_to_move = self.features
        self.features = []
        for feat in features_to_move:
            for child in self.children:
                child.insert(feat)
