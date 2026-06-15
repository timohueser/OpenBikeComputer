from obcm.quadtree import QuadtreeNode
from shapely.geometry import LineString, MultiLineString

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

def test_quadtree_multilinestring_flattening():
    node = QuadtreeNode((0, 0, 1000, 1000))
    # Intersection with a box that splits a line into two parts
    # Square: 0.0 to 0.001 deg (0 to 1000 microdegrees)
    # Line from (-0.001, 0.0005) to (0.002, 0.0005)
    # Box from (0.0002, 0.0, 0.0004, 0.001) - vertical strip
    # Wait, intersection of LineString and Box usually returns LineString or MultiLineString
    
    # Let's use a MultiLineString directly to test flattening
    mls = MultiLineString([
        [(0.0001, 0.0001), (0.0002, 0.0002)],
        [(0.0003, 0.0003), (0.0004, 0.0004)]
    ])
    node.insert({"style_id": 1, "geometry": mls})
    
    # Should be flattened into two features
    assert len(node.features) == 2
    for feat in node.features:
        assert isinstance(feat["geometry"], LineString)

def test_quadtree_split_size():
    # Small chunk size to force split
    node = QuadtreeNode((0, 0, 1000, 1000), chunk_size=50)
    
    # Feature with many points to exceed size
    # Estimate: 8 + (count * 4) > 50 => count * 4 > 42 => count > 10
    line = LineString([(0.0001 * i, 0.0001 * i) for i in range(15)])
    
    # First insert might just fit or trigger split if it's already too big
    node.insert({"style_id": 1, "geometry": line})
    
    # If it split, it should not be a leaf and should have children
    assert node.is_leaf == False
    assert len(node.children) == 4

def test_quadtree_polygon_handling():
    from shapely.geometry import Polygon
    node = QuadtreeNode((0, 0, 1000, 1000))
    poly = Polygon([(0.0001, 0.0001), (0.0005, 0.0001), (0.0005, 0.0005), (0.0001, 0.0005)])
    node.insert({"style_id": 1, "geometry": poly})
    # Should extract exterior as LineString/LinearRing
    assert len(node.features) == 1
    assert node.features[0]["geometry"].geom_type in ['LineString', 'LinearRing']

def test_quadtree_recursion_guard():
    # Force split on a very small box
    node = QuadtreeNode((0, 0, 8, 8), chunk_size=1)
    line = LineString([(0, 0), (0.000005, 0.000005)])
    node.insert({"style_id": 1, "geometry": line})
    # Should NOT split because dimension < 10
    assert node.is_leaf == True

def test_quadtree_geometry_collection():
    from shapely.geometry import GeometryCollection, Point
    node = QuadtreeNode((0, 0, 1000, 1000))
    gc = GeometryCollection([
        LineString([(0.0001, 0.0001), (0.0002, 0.0002)]),
        Point(0.0005, 0.0005) # Should be ignored
    ])
    node.insert({"style_id": 1, "geometry": gc})
    assert len(node.features) == 1
