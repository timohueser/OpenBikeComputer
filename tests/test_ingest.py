from unittest.mock import MagicMock
from obcm.ingest import OSMHandler
from shapely.geometry import LineString

def test_handler_way_extraction():
    config = {"features": {"highway": {"primary": {"id": 10}}}}
    handler = OSMHandler(config)
    
    # Mock an osmium way
    mock_way = MagicMock()
    mock_way.tags = {"highway": "primary"}
    # Mock nodes with lon/lat
    node1 = MagicMock(); node1.lon = 1.0; node1.lat = 1.0
    node2 = MagicMock(); node2.lon = 2.0; node2.lat = 2.0
    mock_way.nodes = [node1, node2]
    
    handler.way(mock_way)
    
    assert len(handler.features) == 1
    assert handler.features[0]["style_id"] == 10
    assert isinstance(handler.features[0]["geometry"], LineString)
