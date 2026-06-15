import struct
import io
import functools

class OBCMReader:
    def __init__(self, stream):
        self.stream = stream
        self._read_header()
        self.styles = {}
        self._read_styles()
        # Instance-level cache to avoid memory leaks
        self.decode_chunk = functools.lru_cache(maxsize=128)(self._decode_chunk)

    def _read_header(self):
        self.stream.seek(0)
        data = self.stream.read(31)
        if len(data) < 31:
            raise ValueError("File too short for header")
        magic, self.version, min_lat, min_lon, max_lat, max_lon, self.style_offset, self.index_offset, self.chunk_size = struct.unpack("<4sBiiiiIIH", data)
        if magic != b"OBCM":
            raise ValueError("Invalid magic bytes")
        
        if self.style_offset < 31 or self.index_offset < 31:
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
        # Read everything after index_offset (includes index and all data chunks)
        all_data = self.stream.read()
        if not all_data:
            self.index = []
            self.index_raw = b""
            return
            
        # Parse everything as uint32 first to allow traversal
        full_arr = struct.unpack(f"<{len(all_data)//4}I", all_data[:(len(all_data)//4)*4])
        
        # Traverse to find the actual index size (max index accessed)
        max_idx = 0
        if full_arr:
            stack = [0]
            visited = {0}
            while stack:
                idx = stack.pop()
                if idx >= len(full_arr): continue
                max_idx = max(max_idx, idx)
                val = full_arr[idx]
                if val & 0x80000000: # Branch
                    child_start = val & 0x7FFFFFFF
                    for i in range(4):
                        c_idx = child_start + i
                        if c_idx < len(full_arr) and c_idx not in visited:
                            visited.add(c_idx)
                            stack.append(c_idx)
        
        index_count = max_idx + 1
        self.index = list(full_arr[:index_count])
        self.index_raw = all_data[:index_count * 4]

    def _decode_chunk(self, chunk_id, node_bbox):
        """
        Reads and decodes a data chunk from the file.
        """
        if not hasattr(self, 'index'):
            self._load_index()

        # Data starts immediately after the Index Block
        data_start_offset = self.index_offset + (len(self.index) * 4)
        
        self.stream.seek(data_start_offset + chunk_id * self.chunk_size)
        chunk_data = self.stream.read(self.chunk_size)
            
        offset = 0
        features = []
        chunk_size = self.chunk_size
        while offset < chunk_size:
            # Check for padding (0xFF) or end of data
            if offset >= len(chunk_data) or chunk_data[offset] == 0xFF:
                break
            
            # Feature Header (12 bytes)
            # StyleID(u8), PtCount(u16), AnchorX(i32), AnchorY(i32), Flags(u8)
            try:
                style_id, ext_pt_count, ax, ay, flags = struct.unpack_from("<BHiiB", chunk_data, offset)
            except struct.error:
                break
            offset += 12
            
            is_16bit = (flags & 0x01) != 0
            is_polygon = (flags & 0x02) != 0
            has_holes = (flags & 0x04) != 0
            
            d_fmt = "h" if is_16bit else "b"
            d_size = 2 if is_16bit else 1
            
            def read_ring(pt_count, current_offset):
                if pt_count == 0:
                    return [], current_offset
                pts = [(node_bbox[0] + ax, node_bbox[1] + ay)]
                prev_x, prev_y = pts[0]
                for _ in range(pt_count - 1):
                    try:
                        dx, dy = struct.unpack_from(f"<{d_fmt}{d_fmt}", chunk_data, current_offset)
                        current_offset += d_size * 2
                        x, y = prev_x + dx, prev_y + dy
                        pts.append((x, y))
                        prev_x, prev_y = x, y
                    except struct.error:
                        break
                return pts, current_offset

            # Read exterior
            exterior, offset = read_ring(ext_pt_count, offset)
            
            interiors = []
            if is_polygon and has_holes:
                if offset < chunk_size:
                    hole_count = chunk_data[offset]
                    offset += 1
                    for _ in range(hole_count):
                        if offset + 2 > chunk_size: break
                        h_pt_count, = struct.unpack_from("<H", chunk_data, offset)
                        offset += 2
                        hole_pts, offset = read_ring(h_pt_count, offset)
                        interiors.append(hole_pts)
            
            if is_polygon:
                features.append({"style_id": style_id, "type": "polygon", "exterior": exterior, "interiors": interiors})
            else:
                features.append({"style_id": style_id, "type": "line", "points": exterior})
                
        return features

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
