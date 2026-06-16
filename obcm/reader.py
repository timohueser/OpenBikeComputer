import struct
import functools


class OBCMReader:
    """Reader for the OBCM v3 (LOD pyramid) format.

    A v3 file has N LOD layers, each its own quadtree index + data chunks. The
    reader keeps one *active* layer at a time; `select_lod_for_mpp` (or passing
    `mpp` to `query_bbox`) picks the layer for the current zoom. Defaults to the
    finest layer.
    """

    def __init__(self, stream):
        self.stream = stream
        self.lods = []
        self._active_lod = None
        self.index = None
        self._read_header()
        self.styles = {}
        self._read_styles()
        # Instance-level cache to avoid memory leaks
        self.decode_chunk = functools.lru_cache(maxsize=128)(self._decode_chunk)

    def _read_header(self):
        self.stream.seek(0)
        head = self.stream.read(30)
        if len(head) < 30:
            raise ValueError("File too short for header")
        if head[:4] != b"OBCM":
            raise ValueError("Invalid magic bytes")
        self.version = head[4]
        if self.version != 3:
            raise ValueError(f"Unsupported OBCM version {self.version} (expected 3)")

        (_magic, _v, min_lat, min_lon, max_lat, max_lon,
         self.style_offset, lod_count, lod_table_offset) = struct.unpack("<4sBiiiiIBI", head)
        # Stored as (min_lon, min_lat, max_lon, max_lat) to match the packer.
        self.global_bbox = (min_lon, min_lat, max_lon, max_lat)

        # LOD table: lod_count x {max_mpp f32, index_off u32, node_count u32, chunk_size u16, chunk_count u32}
        self.stream.seek(lod_table_offset)
        table = self.stream.read(lod_count * 18)
        for k in range(lod_count):
            mpp, idx_off, node_count, cs, chunk_count = struct.unpack_from("<fIIHI", table, k * 18)
            self.lods.append({
                "max_mpp": mpp, "index_offset": idx_off,
                "node_count": node_count, "chunk_size": cs, "chunk_count": chunk_count,
            })
        if not self.lods:
            raise ValueError("No LOD entries")
        self._select_lod(len(self.lods) - 1)  # default to finest detail

    def _select_lod(self, i):
        """Make LOD `i` the active layer."""
        lod = self.lods[i]
        self._active_lod = i
        self.index_offset = lod["index_offset"]
        self.chunk_size = lod["chunk_size"]
        self._index_node_count = lod["node_count"]
        self.index = None  # force reload for the new layer
        if hasattr(self, "decode_chunk"):
            self.decode_chunk.cache_clear()

    def select_lod_for_mpp(self, mpp):
        """Select the finest LOD whose range still covers `mpp` (meters/pixel)."""
        chosen = 0
        for i, lod in enumerate(self.lods):
            if lod["max_mpp"] >= mpp:
                chosen = i
        if chosen != self._active_lod:
            self._select_lod(chosen)

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
            sid, z, color, weight = struct.unpack("<BbHB", data)
            self.styles[sid] = {"z_index": z, "color": color, "weight": weight}

    def _load_index(self):
        # The active LOD's node count is known, so read exactly that many u32s.
        self.stream.seek(self.index_offset)
        raw = self.stream.read(self._index_node_count * 4)
        count = min(self._index_node_count, len(raw) // 4)
        self.index = list(struct.unpack(f"<{count}I", raw[:count * 4]))
        self.index_raw = raw[:count * 4]

    def _decode_chunk(self, chunk_id, node_bbox):
        """Read and decode a data chunk of the active LOD."""
        if self.index is None:
            self._load_index()

        # Data chunks start immediately after this LOD's index block.
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

            # Feature Header (12 bytes): StyleID(u8), PtCount(u16), AnchorX(i32), AnchorY(i32), Flags(u8)
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

            def read_ring(pt_count, current_offset, is_hole=False):
                if pt_count == 0:
                    return [], current_offset

                anchor_x = node_bbox[0] + ax
                anchor_y = node_bbox[1] + ay

                if is_hole:
                    # Holes store ALL points as deltas (first is rel to anchor)
                    pts = []
                    prev_x, prev_y = anchor_x, anchor_y
                    num_deltas = pt_count
                else:
                    # Exterior starts at anchor, then pt_count-1 deltas
                    pts = [(anchor_x, anchor_y)]
                    prev_x, prev_y = anchor_x, anchor_y
                    num_deltas = pt_count - 1

                for _ in range(num_deltas):
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
            exterior, offset = read_ring(ext_pt_count, offset, is_hole=False)

            interiors = []
            if is_polygon and has_holes:
                if offset < chunk_size:
                    hole_count = chunk_data[offset]
                    offset += 1
                    for _ in range(hole_count):
                        if offset + 2 > chunk_size:
                            break
                        h_pt_count, = struct.unpack_from("<H", chunk_data, offset)
                        offset += 2
                        hole_pts, offset = read_ring(h_pt_count, offset, is_hole=True)
                        interiors.append(hole_pts)

            if is_polygon:
                features.append({"style_id": style_id, "type": "polygon", "exterior": exterior, "interiors": interiors})
            else:
                features.append({"style_id": style_id, "type": "line", "points": exterior})

        return features

    def query_bbox(self, query_bbox, mpp=None):
        """
        Returns list of (chunk_id, node_bbox) that intersect query_bbox.
        query_bbox: (min_lon, min_lat, max_lon, max_lat) in microdegrees
        mpp: meters-per-pixel; if given, selects the LOD layer for that zoom.
        """
        if mpp is not None:
            self.select_lod_for_mpp(mpp)
        if self.index is None:
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
        if not (val & 0x80000000):  # Leaf
            if val != 0x7FFFFFFF:  # Not empty
                results.append((val, node_bbox))
        else:  # Branch
            child_start = val & 0x7FFFFFFF
            min_lon, min_lat, max_lon, max_lat = node_bbox
            mid_lon, mid_lat = (min_lon + max_lon) // 2, (min_lat + max_lat) // 2

            # NW, NE, SW, SE order
            children_bboxes = [
                (min_lon, mid_lat, mid_lon, max_lat),  # NW
                (mid_lon, mid_lat, max_lon, max_lat),  # NE
                (min_lon, min_lat, mid_lon, mid_lat),  # SW
                (mid_lon, min_lat, max_lon, mid_lat)   # SE
            ]
            for i, bbox in enumerate(children_bboxes):
                self._query_recursive(child_start + i, bbox, query_bbox, results)

    def _intersects(self, a, b):
        # a, b: (min_lon, min_lat, max_lon, max_lat)
        return not (a[2] < b[0] or a[0] > b[2] or a[3] < b[1] or a[1] > b[3])
