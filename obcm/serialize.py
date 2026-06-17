import struct
from collections import deque
from typing import List, Tuple
from tqdm import tqdm

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
        
        data += struct.pack("<BbHB", 
                           s["id"], 
                           s.get("z_index", 0), 
                           color, 
                           s.get("weight", 1))
    return data

# Max delta (microdegrees) before a segment is densified to keep deltas in range.
_MAX_SEGMENT = 30000


def _densify(p1, p2, out):
    """Append intermediate points between p1 and p2 (then p2 itself) so that no
    single (dx, dy) step exceeds the 16-bit delta range."""
    dx, dy = p2[0] - p1[0], p2[1] - p1[1]
    max_dist = max(abs(dx), abs(dy))
    if max_dist > _MAX_SEGMENT:
        steps = (max_dist // _MAX_SEGMENT) + 1
        for step in range(1, steps):
            t = step / float(steps)
            out.append((int(round(p1[0] + dx * t)), int(round(p1[1] + dy * t))))
    out.append(p2)


def pack_feature(feature: dict, node_bbox: Tuple[int, int, int, int]) -> bytes:
    """
    Pack a single feature into binary format.
    Header (12 bytes): StyleID(u8), PointCount(u16), AnchorX(i32), AnchorY(i32), Flags(u8)
    Followed by: Delta pairs (int8 or int16)
    For Polygons with holes: HoleCount(u8), then for each hole: PointCount(u16), deltas.
    """
    style_id = feature["style_id"]
    geom = feature["geometry"]

    is_polygon = geom.geom_type == 'Polygon'
    flags = 0
    if is_polygon:
        flags |= 0x02 # Bit 1: Polygon
        rings_to_pack = [geom.exterior.coords] + [ring.coords for ring in geom.interiors]
        if len(rings_to_pack) > 1:
            flags |= 0x04 # Bit 2: Has Holes
    else:
        rings_to_pack = [geom.coords]

    # Convert all coords to microdegrees relative to chunk anchor
    anchor_lon, anchor_lat = None, None
    max_delta = 0
    packed_rings = []

    for i, ring in enumerate(rings_to_pack):
        raw_pts = [(int(round(lon * 1e6)), int(round(lat * 1e6))) for lon, lat in ring]

        if i == 0:
            anchor_lon = raw_pts[0][0] - node_bbox[0]
            anchor_lat = raw_pts[0][1] - node_bbox[1]
            start_ref = raw_pts[0]
        else:
            start_ref = (node_bbox[0] + anchor_lon, node_bbox[1] + anchor_lat)

        # Densify long segments: jump from the reference point to the first
        # point, then walk the rest of the ring.
        pts = []
        _densify(start_ref, raw_pts[0], pts)
        for p2 in raw_pts[1:]:
            _densify(pts[-1], p2, pts)

        if i == 0:
            # Exterior ring: first point is the anchor, deltas start from second point
            prev_x, prev_y = pts[0]
            pts_to_delta = pts[1:]
        else:
            # Hole ring: first delta is relative to the anchor
            prev_x, prev_y = start_ref
            pts_to_delta = pts

        deltas = []
        for x, y in pts_to_delta:
            dx, dy = x - prev_x, y - prev_y
            deltas.extend((dx, dy))
            max_delta = max(max_delta, abs(dx), abs(dy))
            prev_x, prev_y = x, y

        packed_rings.append((len(pts), deltas))

    if max_delta > 127:
        flags |= 0x01 # Bit 0: 16-bit deltas
        d_fmt = "h"
    else:
        d_fmt = "b"

    # Header: StyleID(u8), PointCount(u16), AnchorX(i32), AnchorY(i32), Flags(u8)
    # Note: For polygons, PointCount is the exterior ring point count.
    ext_pt_count = packed_rings[0][0]
    header = struct.pack("<BHiiB", style_id, ext_pt_count, anchor_lon, anchor_lat, flags)

    data = header

    # Exterior deltas
    data += struct.pack(f"<{len(packed_rings[0][1])}{d_fmt}", *packed_rings[0][1])

    # Holes
    if flags & 0x04:
        data += struct.pack("<B", len(packed_rings) - 1)
        for pt_count, deltas in packed_rings[1:]:
            data += struct.pack("<H", pt_count)
            data += struct.pack(f"<{len(deltas)}{d_fmt}", *deltas)

    return data

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

def serialize_tree(root, chunk_size: int, desc: str = "Serializing Quadtree"):
    """Flatten one quadtree into (index_bytes, node_count, chunk_bytes, chunk_count)."""
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

    for node in tqdm(all_nodes, desc=desc, unit="node"):
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
    return index_data, len(flat_index), b"".join(data_chunks), len(data_chunks)


# Header sizes (bytes). v4 appends a uint16 marker color to the v3 header.
HEADER_LEN = 32
LOD_ENTRY_LEN = 18


def serialize_lods(lods, config: dict, global_bbox: Tuple[int, int, int, int]) -> bytes:
    """Serialize a pyramid of LOD layers into the v4 .obcm format.

    `lods` is an ordered (coarsest -> finest) list of dicts:
        {"root": QuadtreeNode, "chunk_size": int, "max_mpp": float | None}
    `max_mpp` is the upper bound of the meters-per-pixel range the layer covers;
    `None` means +inf (the coarsest layer, used when fully zoomed out).
    See docs/superpowers/specs/2026-06-16-obcm-lod-design.md.
    """
    style_data = pack_style_dict(config)
    lod_count = len(lods)
    lod_table_offset = HEADER_LEN + len(style_data)

    # User-position marker color (uint16 RGB565), a single global map-presentation
    # property stored in the header. Defaults to bright red, which reads well over
    # both sea and land. Accept an int or a "0x…" string, like pack_style_dict.
    marker_color = config.get("marker", {}).get("color", 0xF800)
    if isinstance(marker_color, str):
        marker_color = int(marker_color, 16)

    # Flatten each layer's tree.
    blocks = []  # (index_bytes, node_count, chunk_bytes, chunk_count, chunk_size, max_mpp)
    for i, lod in enumerate(lods):
        ib, nc, cb, cc = serialize_tree(lod["root"], lod["chunk_size"], desc=f"Serializing LOD {i}")
        blocks.append((ib, nc, cb, cc, lod["chunk_size"], lod["max_mpp"]))

    # Lay out per-layer index+chunk payloads after the LOD table; record offsets.
    cursor = lod_table_offset + lod_count * LOD_ENTRY_LEN
    table = b""
    payload = b""
    for ib, nc, cb, cc, cs, mpp in blocks:
        index_offset = cursor
        mpp_f = float("inf") if mpp is None else float(mpp)
        table += struct.pack("<fIIHI", mpp_f, index_offset, nc, cs, cc)
        payload += ib + cb
        cursor += len(ib) + len(cb)

    # Header: Magic(4), Version(1), BBox(4x i32), StyleOff(4), LODCount(1),
    # LODTableOff(4), MarkerColor(2).
    header = struct.pack("<4sBiiiiIBIH",
                        b"OBCM",
                        0x04,
                        global_bbox[1], global_bbox[0], global_bbox[3], global_bbox[2],
                        HEADER_LEN,
                        lod_count,
                        lod_table_offset,
                        marker_color)

    return header + style_data + table + payload
