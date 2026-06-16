import struct
from obcm.serialize import pack_style_dict, pack_feature, pack_chunk
from shapely.geometry import LineString, Polygon

def test_pack_style_dict():
    config = {
        "features": {
            "highway": {
                "primary": {"id": 10, "z_index": 50, "color": "0xF9A6", "weight": 4}
            }
        }
    }
    data = pack_style_dict(config)
    # Count(1) + ID(1), Z(1), Color(2), Weight(1) = 6 bytes
    assert len(data) == 6
    count = struct.unpack("<B", data[:1])[0]
    id_, z, color, weight = struct.unpack("<BbHB", data[1:]) # Signed z
    assert count == 1
    assert id_ == 10
    assert z == 50
    assert color == 0xF9A6
    assert weight == 4

def test_pack_feature_8bit():
    feature = {
        "style_id": 10,
        "geometry": LineString([(1.0, 1.0), (1.0001, 1.0001)])
    }
    # node_bbox: (1.0, 1.0, 1.01, 1.01) in microdegrees
    bbox = (1000000, 1000000, 1010000, 1010000)
    data = pack_feature(feature, bbox)
    
    # Header(12) + 1 pair of 8-bit deltas (2 bytes) = 14 bytes
    assert len(data) == 14
    style, count, ax, ay, flags = struct.unpack("<BHiiB", data[:12])
    assert style == 10
    assert count == 2
    assert ax == 0
    assert ay == 0
    assert flags == 0 # Line, 8-bit
    
    dx, dy = struct.unpack("<bb", data[12:])
    assert dx == 100 # (1.0001 - 1.0) * 1e6
    assert dy == 100

def test_pack_chunk_padding():
    feature = {
        "style_id": 10,
        "geometry": LineString([(1.0, 1.0), (1.0001, 1.0001)])
    }
    bbox = (1000000, 1000000, 1010000, 1010000)
    chunk = pack_chunk([feature], bbox, chunk_size=32)
    assert len(chunk) == 32
    # Header(12) + 1 pair of 8-bit deltas (2 bytes) = 14 bytes
    assert chunk[14:] == b"\xff" * 18

def test_pack_polygon_small():
    ext = [(0.0, 0.0), (0.0001, 0.0), (0.0001, 0.0001), (0.0, 0.0001)]
    hole = [(0.00002, 0.00002), (0.00008, 0.00002), (0.00008, 0.00008), (0.00002, 0.00008)]
    feature = {"style_id": 20, "geometry": Polygon(ext, [hole])}
    bbox = (0, 0, 200, 200)
    data = pack_feature(feature, bbox)
    style, count, ax, ay, flags = struct.unpack("<BHiiB", data[:12])
    assert style == 20
    assert count == 5 # Shapely Polygons are closed, so 4 points + 1 closing = 5
    assert flags == 0x06 # HasHoles(1), Poly(1), 8-bit(0)
    # 12 header + 4 pairs of 8-bit deltas (8 bytes) = 20. Index 20 is HoleCount.
    assert data[20] == 1 
    # Hole point count (u16) at index 21
    h_count = struct.unpack("<H", data[21:23])[0]
    assert h_count == 5

def test_serialize_lods_header():
    import math
    from obcm.serialize import serialize_lods
    class MockNode:
        def __init__(self):
            self.is_leaf = True
            self.features = []
            self.bbox = (0, 0, 100, 100)

    lods = [{"root": MockNode(), "chunk_size": 2048, "max_mpp": None}]
    config = {"features": {}}
    binary = serialize_lods(lods, config, (0, 0, 100, 100))

    # v3 header(30) + StyleCount(1) + 1 LOD entry(18) + Index(4) = 53
    assert len(binary) == 53
    magic, ver, lat1, lon1, lat2, lon2, s_off, lod_count, lod_tbl = struct.unpack("<4sBiiiiIBI", binary[:30])
    assert magic == b"OBCM"
    assert ver == 3
    assert s_off == 30
    assert lod_count == 1
    assert lod_tbl == 31  # 30 header + 1 style-count byte

    mpp, idx_off, node_count, c_size, chunk_count = struct.unpack_from("<fIIHI", binary, lod_tbl)
    assert math.isinf(mpp)       # coarsest layer
    assert c_size == 2048
    assert node_count == 1       # single empty leaf
    assert chunk_count == 0
    assert idx_off == lod_tbl + 18
