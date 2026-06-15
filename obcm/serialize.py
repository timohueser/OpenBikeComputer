import struct
from collections import deque
from typing import List, Tuple, Dict
from shapely.geometry import LineString

def pack_style_dict(config: dict) -> bytes:
    """
    Pack the style dictionary into binary format.
    Format: Count (uint8), then ID(u8), Z-Index(u8), Color(u16), Weight(u8)
    """
    styles = []
    for feature_type in config.get("features", {}).values():
        for style in feature_type.values():
            styles.append(style)
    
    styles.sort(key=lambda s: s["id"])
    
    data = struct.pack("<B", len(styles))
    for s in styles:
        color = s["color"]
        if isinstance(color, str):
            color = int(color, 16)
        
        data += struct.pack("<BBHB", 
                           s["id"], 
                           s.get("z_index", 0), 
                           color, 
                           s.get("weight", 1))
    return data

def pack_feature(feature: dict, node_bbox: Tuple[int, int, int, int]) -> bytes:
    """
    Pack a single feature into binary format.
    Header (8 bytes): StyleID(u8), PointCount(u16), AnchorX(i16), AnchorY(i16), DeltaFlag(u8)
    Followed by: Delta pairs (int8 or int16)
    """
    style_id = feature["style_id"]
    geom = feature["geometry"]
    coords = list(geom.coords)
    
    # Convert float degrees to microdegrees
    pts = [(int(round(lon * 1e6)), int(round(lat * 1e6))) for lon, lat in coords]
    
    # Anchor is relative to node's min boundaries
    anchor_x = pts[0][0] - node_bbox[0]
    anchor_y = pts[0][1] - node_bbox[1]
    
    deltas = []
    max_delta = 0
    prev_x, prev_y = pts[0]
    
    for x, y in pts[1:]:
        dx, dy = x - prev_x, y - prev_y
        deltas.extend([dx, dy])
        max_delta = max(max_delta, abs(dx), abs(dy))
        prev_x, prev_y = x, y
        
    delta_flag = 1 if max_delta <= 127 else 2
    
    header = struct.pack("<BHhhB", 
                        style_id, 
                        len(pts), 
                        anchor_x, 
                        anchor_y, 
                        delta_flag)
    
    if delta_flag == 1:
        delta_data = struct.pack(f"<{len(deltas)}b", *deltas)
    else:
        delta_data = struct.pack(f"<{len(deltas)}h", *deltas)
        
    return header + delta_data

def pack_chunk(features: List[dict], node_bbox: Tuple[int, int, int, int], chunk_size: int) -> bytes:
    """
    Pack features into a fixed-size chunk, padded with 0xFF.
    """
    data = b""
    for feat in features:
        packed = pack_feature(feat, node_bbox)
        # Ensure we don't overflow the chunk
        if len(data) + len(packed) > chunk_size:
            break
        data += packed
    
    padding = b"\xff" * (chunk_size - len(data))
    return data + padding

def serialize_all(root, config: dict, global_bbox: Tuple[int, int, int, int], chunk_size: int = 4096) -> bytes:
    """
    Serialize the entire quadtree into the .obcm binary format.
    """
    style_data = pack_style_dict(config)
    
    # BFS traversal
    all_nodes = []
    queue = deque([root])
    while queue:
        node = queue.popleft()
        all_nodes.append(node)
        if not node.is_leaf:
            queue.extend(node.children)
            
    flat_index = []
    data_chunks = []
    node_to_idx = {node: i for i, node in enumerate(all_nodes)}
    
    for node in all_nodes:
        if node.is_leaf:
            if not node.features:
                flat_index.append(0x7FFFFFFF)
            else:
                chunk_id = len(data_chunks)
                data_chunks.append(pack_chunk(node.features, node.bbox, chunk_size))
                flat_index.append(chunk_id & 0x7FFFFFFF)
        else:
            first_child_idx = node_to_idx[node.children[0]]
            flat_index.append(first_child_idx | 0x80000000)
            
    index_data = struct.pack(f"<{len(flat_index)}I", *flat_index)
    
    # Header: Magic(4), Version(1), BBox(4x i32), StyleOff(4), IndexOff(4), ChunkSize(2)
    style_offset = 31 
    index_offset = style_offset + len(style_data)
    
    header = struct.pack("<4sBiiiiIIH",
                        b"OBCM",
                        0x01,
                        global_bbox[1], global_bbox[0], global_bbox[3], global_bbox[2],
                        style_offset,
                        index_offset,
                        chunk_size)
    
    full_binary = header + style_data + index_data
    for chunk in data_chunks:
        full_binary += chunk
        
    return full_binary
