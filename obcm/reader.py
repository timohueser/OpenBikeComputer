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
        # Store as (min_lon, min_lat, max_lon, max_lat) to match pipeline's global_bbox order
        self.global_bbox = (min_lon, min_lat, max_lon, max_lat)

    def _read_styles(self):
        self.stream.seek(self.style_offset)
        raw_count = self.stream.read(1)
        if not raw_count:
            return
        count = struct.unpack("<B", raw_count)[0]
        for _ in range(count):
            sid, z, color, weight = struct.unpack("<BBHB", self.stream.read(5))
            self.styles[sid] = {"z_index": z, "color": color, "weight": weight}
