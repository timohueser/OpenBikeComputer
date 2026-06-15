import pytest
import struct
import io
from obcm.reader import OBCMReader

def test_read_header():
    # Mock a minimal OBCM file
    # Magic(4), Ver(1), BBox(4*i32), StyleOff(4), IndexOff(4)
    # Global BBox: minlat=0, minlon=0, maxlat=100, maxlon=100
    # In .obcm header it is stored as: MinLat, MinLon, MaxLat, MaxLon
    data = struct.pack("<4sBiiiiII", b"OBCM", 1, 0, 0, 100, 100, 29, 40)
    stream = io.BytesIO(data)
    reader = OBCMReader(stream)
    assert reader.version == 1
    assert reader.global_bbox == (0, 0, 100, 100) # (min_lon, min_lat, max_lon, max_lat)
