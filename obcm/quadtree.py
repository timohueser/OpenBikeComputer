from shapely.geometry import box, LineString, MultiLineString

class QuadtreeNode:
    def __init__(self, bbox, chunk_size=4096):
        # bbox: (min_lon, min_lat, max_lon, max_lat) in microdegrees
        self.bbox = bbox
        self.chunk_size = chunk_size
        self.features = []
        self.children = []
        self.is_leaf = True

    def insert(self, feature):
        min_lon_f, min_lat_f, max_lon_f, max_lat_f = [c / 1e6 for c in self.bbox]
        q_box = box(min_lon_f, min_lat_f, max_lon_f, max_lat_f)
        
        clipped = feature["geometry"].intersection(q_box)
        if clipped.is_empty:
            return

        if isinstance(clipped, MultiLineString):
            for part in clipped.geoms:
                self._process_clipped({"style_id": feature["style_id"], "geometry": part})
        elif isinstance(clipped, LineString):
            self._process_clipped({"style_id": feature["style_id"], "geometry": clipped})

    def _process_clipped(self, feature):
        if self.is_leaf:
            self.features.append(feature)
            if self.should_split():
                self.split()
        else:
            for child in self.children:
                child.insert(feature)

    def should_split(self):
        width = self.bbox[2] - self.bbox[0]
        height = self.bbox[3] - self.bbox[1]
        if width > 32767 or height > 32767:
            return True
        
        total_size = 0
        for feat in self.features:
            pt_count = len(feat["geometry"].coords)
            # Estimate: 8 byte header + 4 bytes per point (16-bit deltas)
            total_size += 8 + (pt_count * 4) 
        return total_size > self.chunk_size

    def split(self):
        min_lon, min_lat, max_lon, max_lat = self.bbox
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
