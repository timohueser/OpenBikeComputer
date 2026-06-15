import pytest
import struct
import io
from obcm.reader import OBCMReader

def test_read_header():
    # MinLat=10, MinLon=20, MaxLat=30, MaxLon=40
    data = struct.pack("<4sBiiiiII", b"OBCM", 1, 10, 20, 30, 40, 29, 40)
    stream = io.BytesIO(data)
    reader = OBCMReader(stream)
    assert reader.version == 1
    # Store as (min_lon, min_lat, max_lon, max_lat)
    assert reader.global_bbox == (20, 10, 40, 30)

def test_read_styles():
    # Header (29) + StyleCount(1) + 2 Styles (5 bytes each)
    header = struct.pack("<4sBiiiiII", b"OBCM", 1, 0, 0, 100, 100, 29, 40)
    style_data = struct.pack("<B", 2)
    style_data += struct.pack("<BBHB", 10, 50, 0xF9A6, 4)
    style_data += struct.pack("<BBHB", 11, 40, 0xF79E, 3)
    stream = io.BytesIO(header + style_data)
    reader = OBCMReader(stream)
    assert len(reader.styles) == 2
    assert reader.styles[10]["color"] == 0xF9A6
    assert reader.styles[10]["z_index"] == 50
    assert reader.styles[10]["weight"] == 4
    assert reader.styles[11]["color"] == 0xF79E

def test_read_styles_truncated():
    # Header (29) + StyleCount(1)
    header = struct.pack("<4sBiiiiII", b"OBCM", 1, 0, 0, 100, 100, 29, 40)
    # Style count says 2, but only 1 style entry follows
    style_data = struct.pack("<B", 2)
    style_data += struct.pack("<BBHB", 10, 50, 0xF9A6, 4)
    # Missing second style
    stream = io.BytesIO(header + style_data)
    reader = OBCMReader(stream)
    assert len(reader.styles) == 1
    assert 10 in reader.styles

def test_offset_validation():
    # StyleOff < 29
    data = struct.pack("<4sBiiiiII", b"OBCM", 1, 0, 0, 100, 100, 20, 40)
    stream = io.BytesIO(data)
    with pytest.raises(ValueError, match="Offset too small"):
        OBCMReader(stream)

    # IndexOff < 29
    data = struct.pack("<4sBiiiiII", b"OBCM", 1, 0, 0, 100, 100, 29, 20)
    stream = io.BytesIO(data)
    with pytest.raises(ValueError, match="Offset too small"):
        OBCMReader(stream)

def test_spatial_query():
    # Header (29) + StyleCount(1) = 30
    # Index at 30
    # Root branch (bit31=1) at index 0 pointing to child index 1
    # Children at 1, 2, 3, 4 are leaves (bit31=0)
    # Chunk IDs: 0, 1, 2, 3
    index_data = struct.pack("<IIIII", 0x80000001, 0, 1, 2, 3)
    header = struct.pack("<4sBiiiiII", b"OBCM", 1, 0, 0, 100, 100, 29, 30)
    styles = struct.pack("<B", 0)
    
    stream = io.BytesIO(header + styles + index_data)
    reader = OBCMReader(stream)
    
    # Query covering the whole map
    results = reader.query_bbox((0, 0, 100, 100))
    assert len(results) == 4
    # Query only the NW quadrant
    results_nw = reader.query_bbox((10, 60, 40, 90))
    assert len(results_nw) == 1
    assert results_nw[0][0] == 0 # Chunk ID of NW leaf

def test_decode_chunk():
    # 1 feature, 2 points, 8-bit deltas
    # AnchorX=10, AnchorY=20, Deltas=(5, 5)
    f_header = struct.pack("<BHhhB", 10, 2, 10, 20, 1)
    f_deltas = struct.pack("<bb", 5, 5)
    chunk_data = f_header + f_deltas
    chunk_data += b"\xff" * (4096 - len(chunk_data))
    
    # We need to mock a reader with this chunk data at the right offset
    # Index with 1 leaf pointing to chunk 0
    index_data = struct.pack("<I", 0)
    header = struct.pack("<4sBiiiiII", b"OBCM", 1, 0, 0, 100, 100, 29, 30)
    styles = struct.pack("<B", 0)
    
    stream = io.BytesIO(header + styles + index_data + chunk_data)
    reader = OBCMReader(stream)
    
    node_bbox = (0, 0, 100, 100)
    features = reader.decode_chunk(0, node_bbox)
    assert len(features) == 1
    assert features[0]["points"] == [(10, 20), (15, 25)]
