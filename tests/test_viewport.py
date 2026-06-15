import pytest
from obcm.viewport import Viewport

def test_viewport_projection():
    # 60 degrees latitude (cos(60) = 0.5)
    vp = Viewport(800, 600, 60000000)
    vp.camera_lon = 0
    vp.camera_lat = 0
    vp.zoom = 1.0
    
    # Center (0, 0) should be screen (400, 300)
    assert vp.to_screen(0, 0) == (400, 300)
    
    # Lon=100 should be X=400 + 100*0.5 = 450
    assert vp.to_screen(100, 0) == (450, 300)
    
    # Lat=100 should be Y=300 - 100 = 200
    assert vp.to_screen(0, 100) == (400, 200)

def test_viewport_inverse():
    vp = Viewport(800, 600, 0) # cos(0) = 1.0
    vp.camera_lon = 1000
    vp.camera_lat = 2000
    vp.zoom = 2.0
    
    lon, lat = vp.to_map(400, 300)
    assert lon == 1000
    assert lat == 2000
