import pytest
import osmium
from unittest.mock import MagicMock
from obcm.ingest import OSMHandler
from shapely.geometry import LineString, Polygon

def test_handler_way_extraction():
    config = {"features": {"highway": {"primary": {"id": 10}}}}
    handler = OSMHandler(config)
    
    # Mock an osmium way
    mock_way = MagicMock()
    mock_way.tags = MagicMock()
    mock_way.tags.__iter__.return_value = [("highway", "primary")]
    mock_way.tags.get.side_effect = lambda k: "primary" if k == "highway" else None
    
    # Mock nodes with lon/lat
    node1 = MagicMock(); node1.lon = 1.0; node1.lat = 1.0
    node2 = MagicMock(); node2.lon = 2.0; node2.lat = 2.0
    mock_way.nodes = [node1, node2]
    
    handler.way(mock_way)
    
    assert len(handler.features) == 1
    assert handler.features[0]["style_id"] == 10
    assert isinstance(handler.features[0]["geometry"], LineString)

def test_handler_way_too_few_nodes():
    config = {"features": {"highway": {"primary": {"id": 10}}}}
    handler = OSMHandler(config)
    mock_way = MagicMock()
    mock_way.tags = MagicMock()
    mock_way.tags.__iter__.return_value = [("highway", "primary")]
    mock_way.tags.get.side_effect = lambda k: "primary" if k == "highway" else None
    node1 = MagicMock(); node1.lon = 1.0; node1.lat = 1.0
    mock_way.nodes = [node1] # Only 1 node
    handler.way(mock_way)
    assert len(handler.features) == 0

def test_handler_way_no_matching_tags():
    config = {"features": {"highway": {"primary": {"id": 10}}}}
    handler = OSMHandler(config)
    mock_way = MagicMock()
    mock_way.tags = MagicMock()
    mock_way.tags.__iter__.return_value = [("highway", "residential")]
    mock_way.tags.get.side_effect = lambda k: "residential" if k == "highway" else None
    node1 = MagicMock(); node1.lon = 1.0; node1.lat = 1.0
    node2 = MagicMock(); node2.lon = 2.0; node2.lat = 2.0
    mock_way.nodes = [node1, node2]
    handler.way(mock_way)
    assert len(handler.features) == 0

def test_handler_way_invalid_location_error():
    config = {"features": {"highway": {"primary": {"id": 10}}}}
    handler = OSMHandler(config)
    mock_way = MagicMock()
    mock_way.tags = MagicMock()
    mock_way.tags.__iter__.return_value = [("highway", "primary")]
    mock_way.tags.get.side_effect = lambda k: "primary" if k == "highway" else None
    
    mock_way.nodes.__iter__.side_effect = osmium.InvalidLocationError
    
    handler.way(mock_way)
    assert len(handler.features) == 0

def test_handler_coastline_extraction():
    config = {"features": {}}
    handler = OSMHandler(config)
    mock_way = MagicMock()
    mock_way.tags = MagicMock()
    mock_way.tags.get.side_effect = lambda k: "coastline" if k == "natural" else None
    mock_way.tags.__iter__.return_value = [("natural", "coastline")]
    
    node1 = MagicMock(); node1.lon = 1.0; node1.lat = 1.0
    node2 = MagicMock(); node2.lon = 2.0; node2.lat = 2.0
    mock_way.nodes = [node1, node2]
    
    handler.way(mock_way)
    assert len(handler.coastlines) == 1
    assert isinstance(handler.coastlines[0], LineString)

def test_handler_area_extraction():
    config = {"features": {"leisure": {"park": {"id": 20}}}}
    handler = OSMHandler(config)
    
    mock_area = MagicMock()
    mock_area.tags = MagicMock()
    mock_area.tags.__iter__.return_value = [("leisure", "park")]
    mock_area.tags.get.side_effect = lambda k: "park" if k == "leisure" else None
    
    # Mock rings
    outer = [MagicMock(lon=0, lat=0), MagicMock(lon=1, lat=0), MagicMock(lon=1, lat=1), MagicMock(lon=0, lat=1)]
    mock_area.outer_rings.return_value = [outer]
    mock_area.inner_rings.return_value = []
    
    handler.area(mock_area)
    
    assert len(handler.features) == 1
    assert handler.features[0]["style_id"] == 20
    assert isinstance(handler.features[0]["geometry"], Polygon)
    assert handler.features[0]["geometry"].exterior.coords[:] == [(0,0), (1,0), (1,1), (0,1), (0,0)]
