import struct
import io

class OBCMReader:
    def __init__(self, stream):
        self.stream = stream
        self._read_header()
        self.styles = {}
        self._read_styles()

    def _read_header(self):
        self.stream.seek(0)
        data = self.stream.read(29)
        if len(data) < 29:
            raise ValueError("File too short for header")
        magic, self.version, min_lat, min_lon, max_lat, max_lon, self.style_offset, self.index_offset = struct.unpack("<4sBiiiiII", data)
        if magic != b"OBCM":
            raise ValueError("Invalid magic bytes")
        
        if self.style_offset < 29 or self.index_offset < 29:
            raise ValueError("Offset too small")
            
        # Store as (min_lon, min_lat, max_lon, max_lat) to match pipeline's global_bbox order
        self.global_bbox = (min_lon, min_lat, max_lon, max_lat)

    def _read_styles(self):
        self.stream.seek(self.style_offset)
        raw_count = self.stream.read(1)
        if not raw_count:
            return
        count = struct.unpack("<B", raw_count)[0]
        for _ in range(count):
            data = self.stream.read(5)
            if len(data) < 5:
                break
            sid, z, color, weight = struct.unpack("<BBHB", data)
            self.styles[sid] = {"z_index": z, "color": color, "weight": weight}

    def _load_index(self):
        self.stream.seek(self.index_offset)
        # Read the entire index into memory (it's small relative to geometry)
        # We stop at the first non-valid index byte or end of file
        data = self.stream.read()
        if not data:
            self.index = []
            return
        # Index is uint32 array
        count = len(data) // 4
        self.index = list(struct.unpack(f"<{count}I", data[:count*4]))

    def query_bbox(self, query_bbox):
        """
        Returns list of (chunk_id, node_bbox) that intersect query_bbox.
        query_bbox: (min_lon, min_lat, max_lon, max_lat) in microdegrees
        """
        if not hasattr(self, 'index'):
            self._load_index()
        
        results = []
        if self.index:
            self._query_recursive(0, self.global_bbox, query_bbox, results)
        return results

    def _query_recursive(self, node_idx, node_bbox, query_bbox, results):
        if not self._intersects(node_bbox, query_bbox):
            return

        if node_idx >= len(self.index):
            return

        val = self.index[node_idx]
        if not (val & 0x80000000): # Leaf
            if val != 0x7FFFFFFF: # Not empty
                results.append((val, node_bbox))
        else: # Branch
            child_start = val & 0x7FFFFFFF
            min_lon, min_lat, max_lon, max_lat = node_bbox
            mid_lon, mid_lat = (min_lon + max_lon) // 2, (min_lat + max_lat) // 2
            
            # NW, NE, SW, SE order
            children_bboxes = [
                (min_lon, mid_lat, mid_lon, max_lat), # NW
                (mid_lon, mid_lat, max_lon, max_lat), # NE
                (min_lon, min_lat, mid_lon, mid_lat), # SW
                (mid_lon, min_lat, max_lon, mid_lat)  # SE
            ]
            for i, bbox in enumerate(children_bboxes):
                self._query_recursive(child_start + i, bbox, query_bbox, results)

    def _intersects(self, a, b):
        # a, b: (min_lon, min_lat, max_lon, max_lat)
        return not (a[2] < b[0] or a[0] > b[2] or a[3] < b[1] or a[1] > b[3])
