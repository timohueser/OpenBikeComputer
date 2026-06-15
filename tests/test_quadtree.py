from obcm.quadtree import QuadtreeNode
from shapely.geometry import LineString

def test_quadtree_split_dimension():
    # BBox larger than 32767 microdegrees
    node = QuadtreeNode((0, 0, 40000, 40000))
    node.features = [{"style_id": 1, "geometry": LineString([(0.01, 0.01), (0.02, 0.02)])}]
    assert node.should_split() == True

def test_quadtree_insertion():
    node = QuadtreeNode((0, 0, 1000, 1000))
    # Line within BBox (0.0, 0.0 to 0.001, 0.001 degrees)
    line = LineString([(0.0005, 0.0005), (0.0006, 0.0006)])
    node.insert({"style_id": 1, "geometry": line})
    assert len(node.features) == 1
