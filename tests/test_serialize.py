import struct
from obcm.serialize import pack_style_dict, pack_feature, pack_chunk
from shapely.geometry import LineString

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
    count, id_, z, color, weight = struct.unpack("<BBBHB", data)
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
    
    # Header(8) + 1 pair of 8-bit deltas (2 bytes) = 10 bytes
    assert len(data) == 10
    style, count, ax, ay, flag = struct.unpack("<BHhhB", data[:8])
    assert style == 10
    assert count == 2
    assert ax == 0
    assert ay == 0
    assert flag == 1 # 8-bit
    
    dx, dy = struct.unpack("<bb", data[8:])
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
    assert chunk[10:] == b"\xff" * 22
